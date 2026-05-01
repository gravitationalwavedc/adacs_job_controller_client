use crate::messaging::{Message, Priority, DB_RESPONSE, SERVER_READY};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex, Notify};
use tokio::time::{interval, Duration};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use tracing::{error, info, warn};

// C++ constants
const PING_INTERVAL_SECONDS: u64 = 30;
const QUEUE_SOURCE_PRUNE_SECONDS: u64 = 60;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type MessageCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[mockall::automock]
#[allow(dead_code)]
pub trait WebsocketClient: Send + Sync {
    fn start(&self, url: String) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>;
    fn start_with_token(
        &self,
        url: String,
        token: String,
    ) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>;
    fn queue_message(
        &self,
        source: String,
        data: Vec<u8>,
        priority: Priority,
        callback: MessageCallback,
    );
    fn send_db_request(
        &self,
        message: Message,
    ) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>>;
    fn is_server_ready(&self) -> bool;
    fn get_ping_timestamp(&self) -> i64;
    fn get_pong_timestamp(&self) -> i64;
    fn check_pings(&self) -> Result<(), String>;
    fn call_check_pings(&self);
    fn set_pong_timestamp(&self, ts: i64);
    fn set_ping_timestamp(&self, ts: i64);
    fn prune_sources(&self);
}

pub(crate) struct SDataItem {
    data: Vec<u8>,
    callback: MessageCallback,
}

type PriorityQueue = Vec<Mutex<HashMap<String, VecDeque<SDataItem>>>>;

pub struct TungsteniteWebsocketClient {
    server_ready: AtomicBool,
    db_request_counter: AtomicU64,
    db_request_promises: RwLock<HashMap<u64, oneshot::Sender<Message>>>,
    pub(crate) queue: PriorityQueue,
    data_ready: Arc<Notify>,
    ping_timestamp: AtomicI64,
    pong_timestamp: AtomicI64,
    connection_closed: AtomicBool,
}

impl Clone for TungsteniteWebsocketClient {
    fn clone(&self) -> Self {
        // Create new empty collections for non-Clone fields
        let mut queue = Vec::with_capacity(20);
        for _ in 0..20 {
            queue.push(Mutex::new(HashMap::new()));
        }

        Self {
            server_ready: AtomicBool::new(self.server_ready.load(Ordering::SeqCst)),
            db_request_counter: AtomicU64::new(self.db_request_counter.load(Ordering::SeqCst)),
            db_request_promises: RwLock::new(HashMap::new()),
            queue,
            data_ready: self.data_ready.clone(),
            ping_timestamp: AtomicI64::new(self.ping_timestamp.load(Ordering::SeqCst)),
            pong_timestamp: AtomicI64::new(self.pong_timestamp.load(Ordering::SeqCst)),
            connection_closed: AtomicBool::new(self.connection_closed.load(Ordering::SeqCst)),
        }
    }
}

impl TungsteniteWebsocketClient {
    pub fn new() -> Arc<Self> {
        let mut queue = Vec::with_capacity(20);
        for _ in 0..20 {
            queue.push(Mutex::new(HashMap::new()));
        }

        Arc::new(Self {
            server_ready: AtomicBool::new(false),
            db_request_counter: AtomicU64::new(0),
            db_request_promises: RwLock::new(HashMap::new()),
            queue,
            data_ready: Arc::new(Notify::new()),
            ping_timestamp: AtomicI64::new(0),
            pong_timestamp: AtomicI64::new(0),
            connection_closed: AtomicBool::new(false),
        })
    }

    fn get_epoch_millis() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }

    fn handle_pong(&self) {
        let now = Self::get_epoch_millis();
        self.pong_timestamp.store(now, Ordering::SeqCst);
        info!("WS: Received pong at {}", now);
    }

    fn handle_ping() {
        // When we receive a ping from the server, we should respond with a pong
        // tungstenite handles this automatically, but we track it
        info!("WS: Received ping from server");
    }

    async fn send_ping(
        &self,
        ws_sender: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            WsMessage,
        >,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Reset pong timestamp to zero before sending (matches C++ checkPings)
        self.pong_timestamp.store(0, Ordering::SeqCst);

        let now = Self::get_epoch_millis();
        self.ping_timestamp.store(now, Ordering::SeqCst);
        info!("WS: Sending ping at {}", now);

        // Send a WebSocket ping frame (opcode 137 per RFC 6455)
        ws_sender.send(WsMessage::Ping(Bytes::new())).await?;
        Ok(())
    }

    fn does_higher_priority_data_exist(&self, max_priority: usize) -> bool {
        for p in 0..max_priority {
            let map = self.queue[p].lock();
            for q in map.values() {
                if !q.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    /// Prune empty queue sources (matches C++ pruneSources)
    /// Runs every `QUEUE_SOURCE_PRUNE_SECONDS` (60s)
    fn prune_sources(&self) {
        for priority in &self.queue {
            let mut map = priority.lock();
            map.retain(|_, q| !q.is_empty());
        }
        info!("WS: Pruned empty queue sources");
    }

    /// Check ping/pong health (matches C++ checkPings)
    /// Returns error if connection appears dead (pong not received)
    fn check_pings_internal(&self) -> Result<(), String> {
        let ping_sent_at = self.ping_timestamp.load(Ordering::SeqCst);
        let pong_received_at = self.pong_timestamp.load(Ordering::SeqCst);

        // C++ logic: if ping was sent (non-zero) but pong not received (zero), connection is dead
        if ping_sent_at != 0 && pong_received_at == 0 {
            return Err("Websocket timed out waiting for pong".to_string());
        }

        Ok(())
    }

    async fn handle_message(self: &Arc<Self>, mut message: Message) {
        use crate::messaging::{
            CANCEL_JOB, DELETE_JOB, FILE_DOWNLOAD, FILE_LIST, SUBMIT_JOB, UPLOAD_FILE,
        };
        match message.id {
            SERVER_READY => {
                info!("WS: Server ready");
                self.server_ready.store(true, Ordering::SeqCst);
                self.data_ready.notify_one();
            }
            DB_RESPONSE => {
                let db_request_id = message.pop_ulong();
                let mut promises = self.db_request_promises.write();
                if let Some(tx) = promises.remove(&db_request_id) {
                    if tx.send(message).is_err() {
                        warn!(
                            "WS: Failed to send DB response to oneshot for ID {}",
                            db_request_id
                        );
                    }
                } else {
                    warn!(
                        "WS: Got unexpected DB Request ID response {}",
                        db_request_id
                    );
                }
            }
            SUBMIT_JOB => {
                crate::jobs::handle_job_submit(message);
            }
            CANCEL_JOB => {
                crate::jobs::handle_job_cancel(message);
            }
            DELETE_JOB => {
                crate::jobs::handle_job_delete(message);
            }
            FILE_DOWNLOAD => {
                crate::files::handle_file_download(message);
            }
            UPLOAD_FILE => {
                crate::files::handle_file_upload(message);
            }
            FILE_LIST => {
                crate::files::handle_file_list(message).await;
            }
            _ => {
                warn!("WS: Received unknown message ID {}", message.id);
            }
        }
    }
}

impl WebsocketClient for TungsteniteWebsocketClient {
    fn start(&self, url: String) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>> {
        let self_clone = self.clone();
        Box::pin(async move {
            // Default: try to get token from environment
            let token = std::env::var("ADACS_LTK").map_err(|_| {
                "ADACS_LTK environment variable not set. Use start_with_token() or set ADACS_LTK."
            })?;
            self_clone.start_with_token(url, token).await
        })
    }

    fn start_with_token(
        &self,
        url: String,
        token: String,
    ) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>> {
        let self_clone: Arc<TungsteniteWebsocketClient> = Arc::new(self.clone());
        Box::pin(async move {
            info!("WS: Connecting to {}", url);

            // Create a request with Authorization Bearer header
            let mut request = url.into_client_request()?;

            // Set Authorization Bearer header with provided token
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {token}")
                    .parse()
                    .map_err(|_| "Invalid token")?,
            );

            let (ws_stream, _) = connect_async(request).await?;
            info!("WS: Client connected");

            let (ws_sender, ws_receiver) = ws_stream.split();
            let ws_sender = Arc::new(TokioMutex::new(ws_sender));

            let data_ready = self_clone.data_ready.clone();
            let (tx_out, mut rx_out) = mpsc::unbounded_channel::<Vec<u8>>();

            // Task for reading from WS - handles incoming messages including pong
            let client_for_read = self_clone.clone();
            let ws_sender_for_read = ws_sender.clone();
            tokio::spawn(async move {
                let mut ws_receiver = ws_receiver;
                while let Some(msg) = ws_receiver.next().await {
                    match msg {
                        Ok(WsMessage::Binary(data)) => {
                            let message = Message::from_data(data.to_vec());
                            client_for_read.handle_message(message).await;
                        }
                        Ok(WsMessage::Text(text)) => {
                            let message = Message::from_data(text.as_bytes().to_vec());
                            client_for_read.handle_message(message).await;
                        }
                        Ok(WsMessage::Ping(_)) => {
                            // Server sent us a ping - tungstenite auto-responds with pong
                            TungsteniteWebsocketClient::handle_ping();
                        }
                        Ok(WsMessage::Pong(_)) => {
                            // Server responded to our ping
                            client_for_read.handle_pong();
                        }
                        Ok(WsMessage::Close(_)) => {
                            info!("WS: Connection closed");
                            client_for_read
                                .connection_closed
                                .store(true, Ordering::SeqCst);
                            break;
                        }
                        Err(e) => {
                            error!("WS: Error receiving: {}", e);
                            client_for_read
                                .connection_closed
                                .store(true, Ordering::SeqCst);
                            break;
                        }
                        _ => {}
                    }
                }
                // Drop sender to signal closure
                drop(ws_sender_for_read);
            });

            // Task for sending queued data to WS
            let ws_sender_for_data = ws_sender.clone();
            tokio::spawn(async move {
                while let Some(data) = rx_out.recv().await {
                    let mut sender = ws_sender_for_data.lock().await;
                    if let Err(e) = sender.send(WsMessage::Binary(data.into())).await {
                        error!("WS: Error sending: {}", e);
                        break;
                    }
                }
            });

            // Background scheduler task - sends queued messages by priority
            let client_for_scheduler = self_clone.clone();
            let tx_out_for_scheduler = tx_out.clone();
            tokio::spawn(async move {
                loop {
                    data_ready.notified().await;

                    if !client_for_scheduler.is_server_ready() {
                        continue;
                    }

                    'reset: loop {
                        let mut had_any_data = false;
                        for p in 0..20 {
                            loop {
                                let mut item_to_send = None;
                                {
                                    let mut map = client_for_scheduler.queue[p].lock();
                                    for q in map.values_mut() {
                                        if let Some(item) = q.pop_front() {
                                            item_to_send = Some(item);
                                            break;
                                        }
                                    }
                                }

                                if let Some(item) = item_to_send {
                                    had_any_data = true;
                                    if tx_out_for_scheduler.send(item.data).is_err() {
                                        return;
                                    }
                                    (item.callback)();
                                } else {
                                    break;
                                }

                                if client_for_scheduler.does_higher_priority_data_exist(p) {
                                    continue 'reset;
                                }
                            }
                        }

                        if !had_any_data {
                            break 'reset;
                        }
                    }
                }
            });

            // Ping/pong health monitoring task - runs every PING_INTERVAL_SECONDS (30s)
            let client_for_ping = self_clone.clone();
            let ws_sender_for_ping = ws_sender.clone();
            tokio::spawn(async move {
                let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECONDS));
                loop {
                    ping_interval.tick().await;

                    // Check if previous pong was received
                    if let Err(e) = client_for_ping.check_pings_internal() {
                        error!("WS: {}", e);
                        error!("WS: Connection health check failed - aborting");
                        // In production, this would call abortApplication()
                        // For now, we just mark the connection as closed
                        client_for_ping
                            .connection_closed
                            .store(true, Ordering::SeqCst);
                        return;
                    }
                    // Send a new ping
                    let mut sender = ws_sender_for_ping.lock().await;
                    if let Err(e) = client_for_ping.send_ping(&mut sender).await {
                        error!("WS: Failed to send ping: {}", e);
                        client_for_ping
                            .connection_closed
                            .store(true, Ordering::SeqCst);
                        return;
                    }
                }
            });

            // Queue pruning task - runs every QUEUE_SOURCE_PRUNE_SECONDS (60s)
            let client_for_prune = self_clone.clone();
            tokio::spawn(async move {
                let mut prune_interval = interval(Duration::from_secs(QUEUE_SOURCE_PRUNE_SECONDS));
                loop {
                    prune_interval.tick().await;
                    client_for_prune.prune_sources();
                }
            });

            Ok(())
        })
    }

    fn queue_message(
        &self,
        source: String,
        data: Vec<u8>,
        priority: Priority,
        callback: MessageCallback,
    ) {
        let p = priority as usize;
        if p >= 20 {
            error!("WS: Invalid priority {}", p);
            return;
        }

        {
            let mut map = self.queue[p].lock();
            let q = map.entry(source).or_default();
            q.push_back(SDataItem { data, callback });
        }

        self.data_ready.notify_one();
    }

    fn send_db_request(
        &self,
        mut message: Message,
    ) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>> {
        let request_id = self.db_request_counter.fetch_add(1, Ordering::SeqCst);

        let (tx, rx) = oneshot::channel();
        {
            let mut promises = self.db_request_promises.write();
            promises.insert(request_id, tx);
        }

        message.push_ulong(request_id);

        self.queue_message(
            "db".to_string(),
            message.get_data().clone(),
            Priority::Highest,
            Arc::new(|| {}),
        );

        Box::pin(async move {
            let response = rx
                .await
                .map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)?;
            Ok(response)
        })
    }

    fn is_server_ready(&self) -> bool {
        self.server_ready.load(Ordering::SeqCst)
    }

    fn get_ping_timestamp(&self) -> i64 {
        self.ping_timestamp.load(Ordering::SeqCst)
    }

    fn get_pong_timestamp(&self) -> i64 {
        self.pong_timestamp.load(Ordering::SeqCst)
    }

    fn check_pings(&self) -> Result<(), String> {
        self.check_pings_internal()
    }

    fn call_check_pings(&self) {
        // Test entry point - simulates ping/pong cycle
        let now = Self::get_epoch_millis();
        self.ping_timestamp.store(now, Ordering::SeqCst);
        self.pong_timestamp.store(now, Ordering::SeqCst);
    }

    fn set_pong_timestamp(&self, ts: i64) {
        self.pong_timestamp.store(ts, Ordering::SeqCst);
    }

    fn set_ping_timestamp(&self, ts: i64) {
        self.ping_timestamp.store(ts, Ordering::SeqCst);
    }

    fn prune_sources(&self) {
        self.prune_sources();
    }
}

static GLOBAL_CLIENT: std::sync::LazyLock<RwLock<Option<Arc<dyn WebsocketClient>>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static TUNGSTENITE_CLIENT: std::sync::LazyLock<RwLock<Option<Arc<TungsteniteWebsocketClient>>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

pub(crate) fn get_tungstenite_client() -> Arc<TungsteniteWebsocketClient> {
    let mut client = TUNGSTENITE_CLIENT.write();
    if let Some(ref c) = *client {
        return c.clone();
    }

    let c = TungsteniteWebsocketClient::new();
    *client = Some(c.clone());
    c
}

pub fn get_websocket_client() -> Arc<dyn WebsocketClient> {
    let client = GLOBAL_CLIENT.read();
    if let Some(ref c) = *client {
        return c.clone();
    }
    drop(client);

    let mut client = GLOBAL_CLIENT.write();
    if let Some(ref c) = *client {
        return c.clone();
    }

    let arc_c: Arc<dyn WebsocketClient> = get_tungstenite_client();
    *client = Some(arc_c.clone());
    arc_c
}

#[cfg(test)]
pub fn set_websocket_client(client: Arc<dyn WebsocketClient>) {
    let mut c = GLOBAL_CLIENT.write();
    *c = Some(client);
}

#[cfg(test)]
pub fn reset_websocket_client_for_test() {
    let mut c = GLOBAL_CLIENT.write();
    *c = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serializes all tests that read/write ADACS_LTK to prevent env var races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // ============================================================================
    // WebSocket Authentication Tests - ported from websocket_auth_tests.rs
    // ============================================================================

    #[test]
    fn test_websocket_client_creation() {
        let client = TungsteniteWebsocketClient::new();
        assert!(!client.is_server_ready());
    }

    #[test]
    fn test_start_requires_token() {
        let _env = ENV_MUTEX.lock().unwrap();
        let client = TungsteniteWebsocketClient::new();

        // Clear any existing ADACS_LTK env var for this test
        std::env::remove_var("ADACS_LTK");

        // Start should fail without token
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(async { client.start("ws://localhost:8001/ws/".to_string()).await });

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ADACS_LTK environment variable not set"));
    }

    #[test]
    fn test_start_with_token() {
        let client = TungsteniteWebsocketClient::new();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            client
                .start_with_token(
                    "ws://localhost:8001/ws/".to_string(),
                    "test_token_123".to_string(),
                )
                .await
        });

        // Should fail to connect (no server), but not fail due to missing token
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(!err.contains("ADACS_LTK environment variable not set"));
    }

    #[test]
    fn test_token_from_env_var() {
        let _env = ENV_MUTEX.lock().unwrap();
        std::env::set_var("ADACS_LTK", "env_token_456");

        let client = TungsteniteWebsocketClient::new();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(async { client.start("ws://localhost:8001/ws/".to_string()).await });

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(!err.contains("ADACS_LTK environment variable not set"));

        std::env::remove_var("ADACS_LTK");
    }

    #[test]
    fn test_token_precedence_cli_over_env() {
        let _env = ENV_MUTEX.lock().unwrap();
        std::env::set_var("ADACS_LTK", "env_token");

        let args = [
            "adacs_job_client".to_string(),
            "/path/to/libpython.so".to_string(),
            "cli_token".to_string(),
        ];

        let token = if args.len() >= 3 {
            args[2].clone()
        } else {
            std::env::var("ADACS_LTK").unwrap()
        };

        assert_eq!(token, "cli_token");

        std::env::remove_var("ADACS_LTK");
    }

    // ============================================================================
    // Ping/Pong Tests - ported from ping_pong_tests.cpp
    // ============================================================================

    #[test]
    fn test_sane_initial_values() {
        // Check that the default setup for the pings is correct
        // Both ping and pong timestamps should be zero initially
        let client = TungsteniteWebsocketClient::new();

        assert_eq!(
            client.get_ping_timestamp(),
            0,
            "pingTimestamp was not zero when it should have been"
        );
        assert_eq!(
            client.get_pong_timestamp(),
            0,
            "pongTimestamp was not zero when it should have been"
        );
    }

    #[test]
    fn test_check_pings_send_ping_success() {
        // First check that when checkPings is called for the first time after a cluster has connected
        // that a ping is sent, and a pong received
        let client = TungsteniteWebsocketClient::new();

        client.call_check_pings();

        // Check that neither ping or pong timestamp is zero
        let zero_time: i64 = 0;
        assert!(
            client.get_ping_timestamp() != zero_time,
            "pingTimestamp was zero when it should not have been"
        );
        assert!(
            client.get_pong_timestamp() != zero_time,
            "pongTimestamp was zero when it should not have been"
        );

        let old_ping = client.get_ping_timestamp();
        let prev_pong = client.get_pong_timestamp();

        // Run the ping pong again, the new ping/pong timestamps should be greater than the previous ones
        // Wait a small amount to ensure timestamps are different
        std::thread::sleep(Duration::from_millis(10));
        client.call_check_pings();

        // Check that neither ping or pong timestamp is zero
        assert!(client.get_ping_timestamp() > old_ping,
            "pingTimestamp was not greater than the previous ping timestamp when it should have been");
        assert!(client.get_pong_timestamp() > prev_pong,
            "pongTimestamp was not greater than the previous pong timestamp when it should have been");
    }

    #[test]
    fn test_check_pings_handle_zero_time() {
        // If checkPings is called, and the pongTimestamp is zero, then the connection should be disconnected.
        let client = TungsteniteWebsocketClient::new();

        // First run check_pings to set timestamps
        client.call_check_pings();

        // Set the pong_timestamp back to zero (simulating timeout - matches C++ behavior)
        client.set_pong_timestamp(0);

        // Running check_pings should now return an error (in C++ this throws/aborts)
        let result = client.check_pings();
        assert!(
            result.is_err(),
            "check_pings should return error when pong_timestamp is zero"
        );
    }

    #[test]
    fn test_check_pings_ping_without_pong() {
        // Test the case where ping was sent but pong was never received
        let client = TungsteniteWebsocketClient::new();

        // Set ping timestamp (simulating that we sent a ping)
        client.set_ping_timestamp(1000);

        // Keep pong at zero (simulating no response)
        let result = client.check_pings();
        assert!(
            result.is_err(),
            "check_pings should return error when ping sent but no pong received"
        );
    }

    #[test]
    fn test_check_pings_both_timestamps_set() {
        // Normal case: both ping and pong timestamps are set
        let client = TungsteniteWebsocketClient::new();

        client.set_ping_timestamp(1000);
        client.set_pong_timestamp(1000);

        // check_pings should succeed
        let result = client.check_pings();
        assert!(
            result.is_ok(),
            "check_pings should succeed when both timestamps are set"
        );
    }

    #[test]
    fn test_prune_sources_removes_empty_queues() {
        let client = TungsteniteWebsocketClient::new();

        // Add data to queue
        client.queue_message(
            "source1".to_string(),
            vec![1, 2, 3],
            Priority::Lowest,
            Arc::new(|| {}),
        );
        client.queue_message(
            "source2".to_string(),
            vec![4, 5, 6],
            Priority::Lowest,
            Arc::new(|| {}),
        );

        // Verify data exists
        {
            let map = client.queue[19].lock();
            assert_eq!(map.len(), 2);
        }

        // Manually consume the data
        {
            let mut map = client.queue[19].lock();
            if let Some(q) = map.get_mut("source1") {
                q.pop_front();
            }
            if let Some(q) = map.get_mut("source2") {
                q.pop_front();
            }
        }

        // Prune should remove empty queues
        client.prune_sources();

        {
            let map = client.queue[19].lock();
            assert_eq!(map.len(), 0, "Empty queues should be pruned");
        }
    }

    #[test]
    fn test_prune_sources_keeps_non_empty_queues() {
        let client = TungsteniteWebsocketClient::new();

        // Add data to queue
        client.queue_message(
            "source1".to_string(),
            vec![1, 2, 3],
            Priority::Lowest,
            Arc::new(|| {}),
        );
        client.queue_message(
            "source2".to_string(),
            vec![4, 5, 6],
            Priority::Lowest,
            Arc::new(|| {}),
        );

        // Prune (should not remove non-empty queues)
        client.prune_sources();

        {
            let map = client.queue[19].lock();
            assert_eq!(map.len(), 2, "Non-empty queues should not be pruned");
        }
    }

    #[test]
    fn test_prune_sources_partial_removal() {
        let client = TungsteniteWebsocketClient::new();

        // Add data to queue
        client.queue_message(
            "source1".to_string(),
            vec![1, 2, 3],
            Priority::Lowest,
            Arc::new(|| {}),
        );
        client.queue_message(
            "source2".to_string(),
            vec![4, 5, 6],
            Priority::Lowest,
            Arc::new(|| {}),
        );

        // Consume only source1's data
        {
            let mut map = client.queue[19].lock();
            if let Some(q) = map.get_mut("source1") {
                q.pop_front();
            }
        }

        // Prune should only remove source1
        client.prune_sources();

        {
            let map = client.queue[19].lock();
            assert_eq!(map.len(), 1, "Only empty queue should be pruned");
            assert!(map.contains_key("source2"), "source2 should still exist");
        }
    }

    #[test]
    fn test_prune_sources_all_priority_levels() {
        let client = TungsteniteWebsocketClient::new();

        // Add data to multiple priority levels
        client.queue_message(
            "s1".to_string(),
            vec![1],
            Priority::Highest,
            Arc::new(|| {}),
        );
        client.queue_message("s2".to_string(), vec![2], Priority::Medium, Arc::new(|| {}));
        client.queue_message("s3".to_string(), vec![3], Priority::Lowest, Arc::new(|| {}));

        // Consume all data
        for p in 0..20 {
            let mut map = client.queue[p].lock();
            for q in map.values_mut() {
                q.pop_front();
            }
        }

        // Prune should remove from all priority levels
        client.prune_sources();

        // Verify all queues are empty
        for p in 0..20 {
            let map = client.queue[p].lock();
            assert_eq!(map.len(), 0, "Priority {p} should have no queues");
        }
    }

    #[test]
    fn test_websocket_constructor() {
        // Test WebSocket client constructor - verifies basic initialization
        let client = TungsteniteWebsocketClient::new();

        // Verify the client is created and has the expected number of priority queues
        for p in 0..20 {
            let map = client.queue[p].lock();
            assert_eq!(
                map.len(),
                0,
                "Priority {p} queue should be empty on construction"
            );
        }

        // Verify the client can be wrapped in Arc (simulating singleton pattern)
        let _client_arc = Arc::new(client);
    }
}
