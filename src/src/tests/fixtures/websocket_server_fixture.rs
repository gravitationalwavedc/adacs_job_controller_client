use crate::messaging::{Message, Priority, SERVER_READY, SYSTEM_SOURCE};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{self, protocol::Message as WsMessage},
};

pub struct WebsocketServerFixture {
    pub port: u16,
    pub msg_rx: mpsc::UnboundedReceiver<Message>,
    pub msg_tx: mpsc::UnboundedSender<Vec<u8>>,
    inbound_rx: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    outbound_tx: mpsc::UnboundedSender<Message>,
    stop_tx: Arc<Notify>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl WebsocketServerFixture {
    async fn spawn_server(
        port: u16,
        outbound_tx: mpsc::UnboundedSender<Message>,
        inbound_rx: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
        stop_signal: Arc<Notify>,
    ) -> tokio::task::JoinHandle<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut ws_stream = accept_async(stream)
                    .await
                    .expect("Error during the websocket handshake occurred");

                let ready_msg = Message::new(SERVER_READY, Priority::Highest, SYSTEM_SOURCE);
                ws_stream
                    .send(WsMessage::Binary(ready_msg.get_data().clone().into()))
                    .await
                    .unwrap();

                let (ws_sender, ws_receiver) = ws_stream.split();

                let mut ws_receiver = ws_receiver;
                let mut ws_sender = ws_sender;

                tokio::select! {
                    () = stop_signal.notified() => {},
                    res = async {
                        while let Some(msg) = ws_receiver.next().await {
                            let msg = match msg {
                                Ok(msg) => msg,
                                Err(
                                    tungstenite::Error::ConnectionClosed
                                    | tungstenite::Error::Protocol(
                                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                                    ),
                                ) => break,
                                Err(e) => panic!("WS error: {e}"),
                            };
                            if msg.is_binary() {
                                let m = Message::from_data(msg.into_data().to_vec());
                                outbound_tx.send(m).unwrap();
                            }
                        }
                    } => res,
                    res = async {
                        loop {
                            let next = {
                                let mut rx = inbound_rx.lock().await;
                                rx.recv().await
                            };

                            let Some(data) = next else {
                                break;
                            };

                            ws_sender.send(WsMessage::Binary(data.into())).await.unwrap();
                        }
                    } => res,
                }
            }
        })
    }

    pub async fn new() -> Self {
        let (msg_tx_to_test, msg_rx_from_server) = mpsc::unbounded_channel::<Message>();
        let (msg_tx_to_server, msg_rx_from_test) = mpsc::unbounded_channel::<Vec<u8>>();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let inbound_rx = Arc::new(Mutex::new(msg_rx_from_test));
        let stop_tx = Arc::new(Notify::new());
        let handle = Self::spawn_server(
            port,
            msg_tx_to_test.clone(),
            inbound_rx.clone(),
            stop_tx.clone(),
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
        }
    }

    pub fn get_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
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
        self.handle = Some(
            Self::spawn_server(
                self.port,
                self.outbound_tx.clone(),
                self.inbound_rx.clone(),
                self.stop_tx.clone(),
            )
            .await,
        );
    }
}
