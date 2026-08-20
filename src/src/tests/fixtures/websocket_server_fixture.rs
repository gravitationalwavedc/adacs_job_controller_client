use crate::messaging::{Message, Priority, SERVER_READY, SYSTEM_SOURCE};
use futures_util::{SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{self, protocol::Message as WsMessage},
};

const INVALID_SERVER_READY_ID: u32 = SERVER_READY.wrapping_add(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerReadyBehaviour {
    Valid,
    InvalidMessageId,
    NonBinary,
    Withheld,
    PeerEof,
    TransportReset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseHandshakeBehaviour {
    Acknowledge,
    WithholdAcknowledgement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ConnectionTermination {
    None,
    ClientClose,
    StreamEof,
    ResetWithoutClosingHandshake,
    OtherError,
    FixtureStopped,
}

impl ConnectionTermination {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::ClientClose,
            2 => Self::StreamEof,
            3 => Self::ResetWithoutClosingHandshake,
            4 => Self::OtherError,
            5 => Self::FixtureStopped,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WebsocketServerConfig {
    pub server_ready: ServerReadyBehaviour,
    pub close_handshake: CloseHandshakeBehaviour,
    /// When `Some(n)`, the server exits its receive loop after observing `n`
    /// incoming binary messages, dropping the WebSocket half. This is the
    /// task-4 chunk-send-failure injection seam: it lets a regression test
    /// deterministically force the next supervisor chunk send to fail.
    pub drop_after_n_incoming: Option<usize>,
}

impl Default for WebsocketServerConfig {
    fn default() -> Self {
        Self {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct LifecycleBarrier {
    reached: AtomicBool,
    reached_notify: Notify,
    released: AtomicBool,
    release_notify: Notify,
}

impl LifecycleBarrier {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn arrive_and_wait(&self) {
        self.reached.store(true, Ordering::Release);
        self.reached_notify.notify_waiters();
        while !self.released.load(Ordering::Acquire) {
            self.release_notify.notified().await;
        }
    }

    pub async fn wait_until_reached(&self) {
        while !self.reached.load(Ordering::Acquire) {
            self.reached_notify.notified().await;
        }
    }

    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.release_notify.notify_waiters();
    }

    pub fn reset(&self) {
        self.reached.store(false, Ordering::Release);
        self.released.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
struct LifecycleState {
    accepted: AtomicUsize,
    live: AtomicUsize,
    released: AtomicUsize,
    termination: AtomicU8,
    client_close_received: AtomicBool,
    accepted_notify: Notify,
    released_notify: Notify,
    handler_completed_notify: Notify,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self {
            accepted: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
            termination: AtomicU8::new(ConnectionTermination::None as u8),
            client_close_received: AtomicBool::new(false),
            accepted_notify: Notify::new(),
            released_notify: Notify::new(),
            handler_completed_notify: Notify::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WebsocketLifecycleObserver {
    state: Arc<LifecycleState>,
}

impl WebsocketLifecycleObserver {
    pub fn accepted_connections(&self) -> usize {
        self.state.accepted.load(Ordering::Acquire)
    }

    pub fn live_connections(&self) -> usize {
        self.state.live.load(Ordering::Acquire)
    }

    pub fn released_connections(&self) -> usize {
        self.state.released.load(Ordering::Acquire)
    }

    pub fn client_close_received(&self) -> bool {
        self.state.client_close_received.load(Ordering::Acquire)
    }

    pub fn termination(&self) -> ConnectionTermination {
        ConnectionTermination::from_u8(self.state.termination.load(Ordering::Acquire))
    }

    pub async fn wait_for_accepted(&self, target: usize) {
        while self.accepted_connections() < target {
            self.state.accepted_notify.notified().await;
        }
    }

    pub async fn wait_for_released(&self, target: usize) {
        while self.released_connections() < target {
            self.state.released_notify.notified().await;
        }
    }

    pub async fn wait_for_handler_completion(&self, released_baseline: usize) {
        while self.released_connections() <= released_baseline {
            self.state.handler_completed_notify.notified().await;
        }
    }
}

struct LiveConnectionGuard {
    state: Arc<LifecycleState>,
}

impl LiveConnectionGuard {
    fn accepted(state: Arc<LifecycleState>) -> Self {
        state.accepted.fetch_add(1, Ordering::AcqRel);
        state.live.fetch_add(1, Ordering::AcqRel);
        state.accepted_notify.notify_waiters();
        Self { state }
    }
}

impl Drop for LiveConnectionGuard {
    fn drop(&mut self) {
        self.state.live.fetch_sub(1, Ordering::AcqRel);
        self.state.released.fetch_add(1, Ordering::AcqRel);
        self.state.released_notify.notify_waiters();
        self.state.handler_completed_notify.notify_waiters();
    }
}

pub struct WebsocketServerFixture {
    pub port: u16,
    pub msg_rx: mpsc::UnboundedReceiver<Message>,
    pub msg_tx: mpsc::UnboundedSender<Vec<u8>>,
    inbound_rx: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    outbound_tx: mpsc::UnboundedSender<Message>,
    stop_tx: Arc<Notify>,
    handle: Option<tokio::task::JoinHandle<()>>,
    config: WebsocketServerConfig,
    lifecycle: Arc<LifecycleState>,
    close_tx: mpsc::UnboundedSender<oneshot::Sender<()>>,
    reset_tx: mpsc::UnboundedSender<oneshot::Sender<()>>,
    pub final_send_barrier: Arc<LifecycleBarrier>,
    pub zero_byte_eof_barrier: Arc<LifecycleBarrier>,
}

impl WebsocketServerFixture {
    fn reset_transport(stream: TcpStream) {
        let std_stream = stream
            .into_std()
            .expect("failed to convert fixture stream for transport reset");
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;

            let linger = libc::linger {
                l_onoff: 1,
                l_linger: 0,
            };
            // SAFETY: `std_stream` owns a valid socket descriptor and `linger`
            // remains alive for the duration of this setsockopt call.
            let result = unsafe {
                libc::setsockopt(
                    std_stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_LINGER,
                    (&raw const linger).cast::<libc::c_void>(),
                    std::mem::size_of::<libc::linger>() as libc::socklen_t,
                )
            };
            assert_eq!(result, 0, "failed to configure fixture transport reset");
        }
        drop(std_stream);
    }

    fn record_termination(state: &LifecycleState, termination: ConnectionTermination) {
        state
            .termination
            .store(termination as u8, Ordering::Release);
    }

    async fn send_server_ready(
        stream: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
        behaviour: ServerReadyBehaviour,
    ) -> bool {
        match behaviour {
            ServerReadyBehaviour::Valid => {
                let ready = Message::new(SERVER_READY, Priority::Highest, SYSTEM_SOURCE);
                stream
                    .send(WsMessage::Binary(ready.get_data().clone().into()))
                    .await
                    .is_ok()
            }
            ServerReadyBehaviour::InvalidMessageId => {
                let ready = Message::new(INVALID_SERVER_READY_ID, Priority::Highest, SYSTEM_SOURCE);
                stream
                    .send(WsMessage::Binary(ready.get_data().clone().into()))
                    .await
                    .is_ok()
            }
            ServerReadyBehaviour::NonBinary => stream
                .send(WsMessage::Text("SERVER_READY".into()))
                .await
                .is_ok(),
            ServerReadyBehaviour::Withheld => true,
            ServerReadyBehaviour::PeerEof | ServerReadyBehaviour::TransportReset => false,
        }
    }

    async fn spawn_server(
        port: u16,
        outbound_tx: mpsc::UnboundedSender<Message>,
        inbound_rx: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
        close_rx: Arc<Mutex<mpsc::UnboundedReceiver<oneshot::Sender<()>>>>,
        reset_rx: Arc<Mutex<mpsc::UnboundedReceiver<oneshot::Sender<()>>>>,
        stop_signal: Arc<Notify>,
        config: WebsocketServerConfig,
        lifecycle: Arc<LifecycleState>,
    ) -> tokio::task::JoinHandle<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let _live_guard = LiveConnectionGuard::accepted(lifecycle.clone());
            let incoming_count = AtomicUsize::new(0);

            if config.server_ready == ServerReadyBehaviour::TransportReset {
                Self::reset_transport(stream);
                Self::record_termination(
                    &lifecycle,
                    ConnectionTermination::ResetWithoutClosingHandshake,
                );
                return;
            }

            let mut ws_stream = accept_async(stream)
                .await
                .expect("Error during the websocket handshake occurred");

            if config.server_ready == ServerReadyBehaviour::PeerEof {
                Self::record_termination(&lifecycle, ConnectionTermination::StreamEof);
                return;
            }

            if !Self::send_server_ready(&mut ws_stream, config.server_ready).await {
                Self::record_termination(&lifecycle, ConnectionTermination::OtherError);
                return;
            }

            let (mut ws_sender, mut ws_receiver) = ws_stream.split();
            let mut reset_requested = false;
            let mut reset_ack: Option<oneshot::Sender<()>> = None;
            loop {
                tokio::select! {
                    () = stop_signal.notified() => {
                        Self::record_termination(
                            &lifecycle,
                            ConnectionTermination::FixtureStopped,
                        );
                        break;
                    }
                    incoming = ws_receiver.next() => {
                        match incoming {
                            Some(Ok(WsMessage::Binary(data))) => {
                                let message = Message::from_data(data.to_vec());
                                let _ = outbound_tx.send(message);
                                if let Some(limit) = config.drop_after_n_incoming {
                                    let count = incoming_count.fetch_add(1, Ordering::AcqRel) + 1;
                                    if count >= limit {
                                        // Drop the WebSocket half so the
                                        // supervisor's next send fails; this
                                        // is the task-4 chunk-send-failure
                                        // regression seam.
                                        Self::record_termination(
                                            &lifecycle,
                                            ConnectionTermination::OtherError,
                                        );
                                        break;
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Close(frame))) => {
                                lifecycle.client_close_received.store(true, Ordering::Release);
                                Self::record_termination(
                                    &lifecycle,
                                    ConnectionTermination::ClientClose,
                                );
                                if config.close_handshake == CloseHandshakeBehaviour::Acknowledge {
                                    let _ = ws_sender.send(WsMessage::Close(frame)).await;
                                    let _ = ws_sender.flush().await;
                                    break;
                                }
                                stop_signal.notified().await;
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(tungstenite::Error::Protocol(
                                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                            ))) => {
                                Self::record_termination(
                                    &lifecycle,
                                    ConnectionTermination::ResetWithoutClosingHandshake,
                                );
                                break;
                            }
                            Some(Err(tungstenite::Error::ConnectionClosed)) | None => {
                                Self::record_termination(
                                    &lifecycle,
                                    ConnectionTermination::StreamEof,
                                );
                                break;
                            }
                            Some(Err(_)) => {
                                Self::record_termination(
                                    &lifecycle,
                                    ConnectionTermination::OtherError,
                                );
                                break;
                            }
                        }
                    }
                    outbound = async {
                        let mut rx = inbound_rx.lock().await;
                        rx.recv().await
                    } => {
                        let Some(data) = outbound else {
                            break;
                        };
                        if ws_sender.send(WsMessage::Binary(data.into())).await.is_err() {
                            Self::record_termination(
                                &lifecycle,
                                ConnectionTermination::OtherError,
                            );
                            break;
                        }
                    }
                    close = async {
                        let mut rx = close_rx.lock().await;
                        rx.recv().await
                    } => {
                        if let Some(ack_tx) = close {
                            let result = ws_sender.send(WsMessage::Close(None)).await;
                            let _ = ack_tx.send(());
                            if result.is_err() {
                                Self::record_termination(
                                    &lifecycle,
                                    ConnectionTermination::OtherError,
                                );
                                break;
                            }
                        }
                    }
                    reset = async {
                        let mut rx = reset_rx.lock().await;
                        rx.recv().await
                    } => {
                        if let Some(ack_tx) = reset {
                            reset_requested = true;
                            reset_ack = Some(ack_tx);
                            break;
                        }
                    }
                }
            }

            if reset_requested {
                // Recombine the split halves and reset the transport with
                // SO_LINGER=0 so the supervisor's next send fails with a
                // connection reset. This is the deterministic
                // Close-send-failure-after-success fixture control.
                if let Ok(ws) = ws_sender.reunite(ws_receiver) {
                    let stream = ws.into_inner();
                    Self::reset_transport(stream);
                }
                Self::record_termination(
                    &lifecycle,
                    ConnectionTermination::ResetWithoutClosingHandshake,
                );
                if let Some(ack_tx) = reset_ack {
                    let _ = ack_tx.send(());
                }
            }
        })
    }

    pub async fn new() -> Self {
        Self::with_config(WebsocketServerConfig::default()).await
    }

    pub async fn with_config(config: WebsocketServerConfig) -> Self {
        let (msg_tx_to_test, msg_rx_from_server) = mpsc::unbounded_channel::<Message>();
        let (msg_tx_to_server, msg_rx_from_test) = mpsc::unbounded_channel::<Vec<u8>>();
        let (close_tx, close_rx_from_test) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
        let (reset_tx, reset_rx_from_test) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let inbound_rx = Arc::new(Mutex::new(msg_rx_from_test));
        let close_rx = Arc::new(Mutex::new(close_rx_from_test));
        let reset_rx = Arc::new(Mutex::new(reset_rx_from_test));
        let stop_tx = Arc::new(Notify::new());
        let lifecycle = Arc::new(LifecycleState::default());
        let handle = Self::spawn_server(
            port,
            msg_tx_to_test.clone(),
            inbound_rx.clone(),
            close_rx.clone(),
            reset_rx.clone(),
            stop_tx.clone(),
            config.clone(),
            lifecycle.clone(),
        )
        .await;

        Self {
            port,
            msg_rx: msg_rx_from_server,
            msg_tx: msg_tx_to_server,
            inbound_rx,
            outbound_tx: msg_tx_to_test,
            stop_tx,
            handle: Some(handle),
            config,
            lifecycle,
            close_tx,
            reset_tx,
            final_send_barrier: LifecycleBarrier::new(),
            zero_byte_eof_barrier: LifecycleBarrier::new(),
        }
    }

    pub fn get_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    pub fn lifecycle(&self) -> WebsocketLifecycleObserver {
        WebsocketLifecycleObserver {
            state: self.lifecycle.clone(),
        }
    }

    /// Inject a genuine WebSocket Close frame from the fixture peer. Awaits
    /// until the fixture has written the Close frame to the transport so a
    /// test can deterministically queue a peer Close before releasing a
    /// lifecycle barrier.
    pub async fn send_peer_close(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.close_tx.send(ack_tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), ack_rx).await;
    }

    /// Reset the fixture peer's transport (`SO_LINGER=0`) so the supervisor's
    /// next send fails with a connection reset. Awaits until the reset has
    /// been applied. This is the deterministic Close-send-failure-after-success
    /// fixture control.
    pub async fn reset_connection(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.reset_tx.send(ack_tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), ack_rx).await;
    }

    pub async fn stop(&mut self) {
        self.stop_tx.notify_waiters();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }

    pub async fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }

        self.stop_tx = Arc::new(Notify::new());
        let (close_tx, close_rx_from_test) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
        let (reset_tx, reset_rx_from_test) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
        self.close_tx = close_tx;
        self.reset_tx = reset_tx;
        let close_rx = Arc::new(Mutex::new(close_rx_from_test));
        let reset_rx = Arc::new(Mutex::new(reset_rx_from_test));
        self.handle = Some(
            Self::spawn_server(
                self.port,
                self.outbound_tx.clone(),
                self.inbound_rx.clone(),
                close_rx,
                reset_rx,
                self.stop_tx.clone(),
                self.config.clone(),
                self.lifecycle.clone(),
            )
            .await,
        );
    }
}
