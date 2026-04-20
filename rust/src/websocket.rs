use crate::messaging::{Message, Priority, SERVER_READY, DB_RESPONSE};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Notify, Mutex as TokioMutex};
use tokio::time::{interval, Duration};
use futures_util::{StreamExt, SinkExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use bytes::Bytes;
use log::{info, error, warn};
use std::pin::Pin;
use std::future::Future;

// C++ constants
const PING_INTERVAL_SECONDS: u64 = 30;
const QUEUE_SOURCE_PRUNE_SECONDS: u64 = 60;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type MessageCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[mockall::automock]
pub trait WebsocketClient: Send + Sync {
    fn start(&self, url: String) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>;
    fn queue_message(&self, source: String, data: Vec<u8>, priority: Priority, callback: MessageCallback);
    fn send_db_request(&self, message: Message) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>>;
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
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn handle_pong(&self) {
        let now = Self::get_epoch_millis();
        self.pong_timestamp.store(now, Ordering::SeqCst);
        info!("WS: Received pong at {}", now);
    }

    fn handle_ping(&self) {
        // When we receive a ping from the server, we should respond with a pong
        // tungstenite handles this automatically, but we track it
        info!("WS: Received ping from server");
    }

    async fn send_ping(&self, ws_sender: &mut futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, WsMessage>) -> Result<(), Box<dyn Error + Send + Sync>> {
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
    /// Runs every QUEUE_SOURCE_PRUNE_SECONDS (60s)
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
        let ping_ts = self.ping_timestamp.load(Ordering::SeqCst);
        let pong_ts = self.pong_timestamp.load(Ordering::SeqCst);

        // C++ logic: if ping was sent (non-zero) but pong not received (zero), connection is dead
        if ping_ts != 0 && pong_ts == 0 {
            return Err("Websocket timed out waiting for pong".to_string());
        }

        Ok(())
    }

    async fn handle_message(self: &Arc<Self>, mut message: Message) {
        use crate::messaging::{SUBMIT_JOB, CANCEL_JOB, DELETE_JOB, FILE_DOWNLOAD, UPLOAD_FILE, FILE_LIST};
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
                    if let Err(_) = tx.send(message) {
                        warn!("WS: Failed to send DB response to oneshot for ID {}", db_request_id);
                    }
                } else {
                    warn!("WS: Got unexpected DB Request ID response {}", db_request_id);
                }
            }
            SUBMIT_JOB => {
                crate::jobs::handle_job_submit(message).await;
            }
            CANCEL_JOB => {
                crate::jobs::handle_job_cancel(message).await;
            }
            DELETE_JOB => {
                crate::jobs::handle_job_delete(message).await;
            }
            FILE_DOWNLOAD => {
                crate::files::handle_file_download(message).await;
            }
            UPLOAD_FILE => {
                crate::files::handle_file_upload(message).await;
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
        let client = get_tungstenite_client();
        Box::pin(async move {
            let (ws_stream, _) = connect_async(&url).await?;
            info!("WS: Client connected to {}", url);

            let (ws_sender, mut ws_receiver) = ws_stream.split();
            let ws_sender = Arc::new(TokioMutex::new(ws_sender));

            let data_ready = client.data_ready.clone();
            let (tx_out, mut rx_out) = mpsc::unbounded_channel::<Vec<u8>>();

            // Task for reading from WS - handles incoming messages including pong
            let client_for_read = client.clone();
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
                            client_for_read.handle_ping();
                        }
                        Ok(WsMessage::Pong(_)) => {
                            // Server responded to our ping
                            client_for_read.handle_pong();
                        }
                        Ok(WsMessage::Close(_)) => {
                            info!("WS: Connection closed");
                            client_for_read.connection_closed.store(true, Ordering::SeqCst);
                            break;
                        }
                        Err(e) => {
                            error!("WS: Error receiving: {}", e);
                            client_for_read.connection_closed.store(true, Ordering::SeqCst);
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
            let client_for_scheduler = client.clone();
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
                                    if let Err(_) = tx_out_for_scheduler.send(item.data) {
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
            let client_for_ping = client.clone();
            let ws_sender_for_ping = ws_sender.clone();
            tokio::spawn(async move {
                let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECONDS));
                loop {
                    ping_interval.tick().await;

                    // Check if previous pong was received
                    match client_for_ping.check_pings_internal() {
                        Err(e) => {
                            error!("WS: {}", e);
                            error!("WS: Connection health check failed - aborting");
                            // In production, this would call abortApplication()
                            // For now, we just mark the connection as closed
                            client_for_ping.connection_closed.store(true, Ordering::SeqCst);
                            return;
                        }
                        Ok(_) => {
                            // Send a new ping
                            let mut sender = ws_sender_for_ping.lock().await;
                            if let Err(e) = client_for_ping.send_ping(&mut sender).await {
                                error!("WS: Failed to send ping: {}", e);
                                client_for_ping.connection_closed.store(true, Ordering::SeqCst);
                                return;
                            }
                        }
                    }
                }
            });

            // Queue pruning task - runs every QUEUE_SOURCE_PRUNE_SECONDS (60s)
            let client_for_prune = client.clone();
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

    fn queue_message(&self, source: String, data: Vec<u8>, priority: Priority, callback: MessageCallback) {
        let p = priority as usize;
        if p >= 20 {
            error!("WS: Invalid priority {}", p);
            return;
        }

        {
            let mut map = self.queue[p].lock();
            let q = map.entry(source).or_insert_with(VecDeque::new);
            q.push_back(SDataItem { data, callback });
        }

        self.data_ready.notify_one();
    }

    fn send_db_request(&self, mut message: Message) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>> {
        let request_id = self.db_request_counter.fetch_add(1, Ordering::SeqCst);

        let (tx, rx) = oneshot::channel();
        {
            let mut promises = self.db_request_promises.write();
            promises.insert(request_id, tx);
        }

        message.push_ulong(request_id);

        self.queue_message("db".to_string(), message.get_data().clone(), Priority::Highest, Arc::new(|| {}));

        Box::pin(async move {
            let response = rx.await.map_err(|e| Box::new(e) as Box<dyn Error + Send + Sync>)?;
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
        self.prune_sources()
    }
}

lazy_static::lazy_static! {
    static ref GLOBAL_CLIENT: RwLock<Option<Arc<dyn WebsocketClient>>> = RwLock::new(None);
    static ref TUNGSTENITE_CLIENT: RwLock<Option<Arc<TungsteniteWebsocketClient>>> = RwLock::new(None);
}

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

pub fn set_websocket_client(client: Arc<dyn WebsocketClient>) {
    let mut c = GLOBAL_CLIENT.write();
    *c = Some(client);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prune_sources() {
        let client = TungsteniteWebsocketClient::new();

        // Add some data to queue
        client.queue_message("source1".to_string(), vec![1, 2, 3], Priority::Lowest, Arc::new(|| {}));
        client.queue_message("source2".to_string(), vec![4, 5, 6], Priority::Lowest, Arc::new(|| {}));

        // Verify data exists
        {
            let map = client.queue[19].lock();
            assert_eq!(map.len(), 2);
        }

        // Prune (should not remove non-empty queues)
        client.prune_sources();

        {
            let map = client.queue[19].lock();
            assert_eq!(map.len(), 2);
        }
    }
}
