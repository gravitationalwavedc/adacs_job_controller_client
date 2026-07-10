use crate::messaging::{Message, Priority, DB_RESPONSE, SERVER_READY};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::{interval, timeout, Duration};
use tokio_tungstenite::{
    connect_async, tungstenite::client::IntoClientRequest,
    tungstenite::protocol::Message as WsMessage,
};
use tracing::{debug, error, info, trace, warn};

// C++ constants
const PING_INTERVAL_SECONDS: u64 = 30;
const WRITER_FLUSH_INTERVAL_SECONDS: u64 = 5;
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
        reconnectable: bool,
    ) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>;
    fn queue_message(&self, source: String, data: Vec<u8>, priority: Priority);
    fn send_db_request(
        &self,
        message: Message,
    ) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>>;
    fn is_server_ready(&self) -> bool;
    fn is_connection_closed(&self) -> bool;
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
}

type PriorityQueue = Vec<Arc<Mutex<HashMap<String, VecDeque<SDataItem>>>>>;

#[derive(Clone)]
struct ConnectionConfig {
    url: String,
    token: String,
}

pub struct TungsteniteWebsocketClient {
    server_ready: AtomicBool,
    db_request_counter: AtomicU32,
    db_request_promises: Arc<RwLock<HashMap<u32, oneshot::Sender<Message>>>>,
    pub(crate) queue: PriorityQueue,
    data_ready: Arc<Notify>,
    ping_timestamp: AtomicI64,
    pong_timestamp: AtomicI64,
    connection_closed: AtomicBool,
    reconnectable: AtomicBool,
    supervisor_started: AtomicBool,
    connection_id: AtomicU64,
    connection_config: Arc<RwLock<Option<ConnectionConfig>>>,
    last_update_check: AtomicI64,
}

impl TungsteniteWebsocketClient {
    fn new_internal() -> Self {
        let mut queue = Vec::with_capacity(20);
        for _ in 0..20 {
            queue.push(Arc::new(Mutex::new(HashMap::new())));
        }

        Self {
            server_ready: AtomicBool::new(false),
            db_request_counter: AtomicU32::new(0),
            db_request_promises: Arc::new(RwLock::new(HashMap::new())),
            queue,
            data_ready: Arc::new(Notify::new()),
            ping_timestamp: AtomicI64::new(0),
            pong_timestamp: AtomicI64::new(0),
            connection_closed: AtomicBool::new(false),
            reconnectable: AtomicBool::new(false),
            supervisor_started: AtomicBool::new(false),
            connection_id: AtomicU64::new(0),
            connection_config: Arc::new(RwLock::new(None)),
            last_update_check: AtomicI64::new(0),
        }
    }

    pub fn new() -> Arc<Self> {
        Arc::new(Self::new_internal())
    }

    fn get_epoch_millis() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }

    fn handle_pong(&self, connection_id: u64) {
        if self.connection_id.load(Ordering::SeqCst) != connection_id {
            return;
        }
        let now = Self::get_epoch_millis();
        let ping_ts = self.ping_timestamp.load(Ordering::SeqCst);
        let latency = if ping_ts != 0 { now - ping_ts } else { -1 };
        self.pong_timestamp.store(now, Ordering::SeqCst);
        debug!("WS: Received pong at {} (latency: {}ms)", now, latency);
    }

    fn handle_ping() {
        // When we receive a ping from the server, we should respond with a pong
        // tungstenite handles this automatically, but we track it
        trace!("WS: Received ping from server");
    }

    /// Returns true if any priority queue below `max_priority` holds data, or if
    /// any such priority lock is currently held by another thread. Uses
    /// `try_lock` so the async scheduler never blocks on a `parking_lot` mutex:
    /// contention is treated as "data exists" (pessimistic), which preserves
    /// the existing reset-to-priority-0 fairness behaviour at the cost of at
    /// most one extra empty priority scan.
    fn does_higher_priority_data_exist(&self, max_priority: usize) -> bool {
        for p in 0..max_priority {
            let Some(map) = self.queue[p].try_lock() else {
                return true;
            };
            for q in map.values() {
                if !q.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn clear_queued_messages(&self) -> usize {
        let mut total_cleared = 0;
        let mut skipped = 0;
        for (p, priority) in self.queue.iter().enumerate() {
            // try_lock avoids blocking an async caller (e.g. handle_disconnect
            // running from the read loop) when another task still holds the
            // priority mutex mid-cycle. Contended priorities hold stale data
            // that the next connection cycle will clear naturally.
            let Some(mut map) = priority.try_lock() else {
                skipped += 1;
                continue;
            };
            let count: usize = map.values().map(std::collections::VecDeque::len).sum();
            total_cleared += count;
            if count > 0 {
                debug!("WS: Clearing {} messages from priority {}", count, p);
            }
            map.clear();
        }
        if skipped > 0 {
            debug!(
                "WS: Skipped {} contended priority queue(s) during disconnect clear (stale data will be cleared on reconnect)",
                skipped
            );
        }
        debug!("WS: Cleared {} total queued messages", total_cleared);
        total_cleared
    }

    /// Prune empty queue sources (matches C++ pruneSources)
    /// Runs every `QUEUE_SOURCE_PRUNE_SECONDS` (60s)
    fn prune_sources(&self) {
        for priority in &self.queue {
            let mut map = priority.lock();
            map.retain(|_, q| !q.is_empty());
        }
        debug!("WS: Pruned empty queue sources");
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

    fn handle_disconnect(&self, connection_id: u64, disconnect_notify: &Arc<Notify>) {
        if self.connection_id.load(Ordering::SeqCst) != connection_id {
            debug!(
                "WS: Ignoring disconnect signal for old connection (id={}, current={})",
                connection_id,
                self.connection_id.load(Ordering::SeqCst)
            );
            return;
        }

        let was_closed = self.connection_closed.swap(true, Ordering::SeqCst);
        self.server_ready.store(false, Ordering::SeqCst);
        let cleared_count = self.clear_queued_messages();
        debug!(
            "WS: Disconnect handling (id={}) - was_closed={}, cleared {} queued messages",
            connection_id, was_closed, cleared_count
        );

        let promises = std::mem::take(&mut *self.db_request_promises.write());
        let pending_requests = promises.len();
        debug!(
            "WS: Disconnect - dropping {} pending DB request promises",
            pending_requests
        );
        drop(promises);

        if !was_closed {
            debug!("WS: Notifying disconnect waiters");
            disconnect_notify.notify_waiters();
            if !self.reconnectable.load(Ordering::SeqCst) {
                debug!("WS: Notifying shutdown (not reconnectable)");
                get_shutdown_notify().notify_waiters();
            }
        }
    }

    fn load_connection_config(&self) -> Option<ConnectionConfig> {
        self.connection_config.read().clone()
    }

    fn reconnect_delay(attempt: u32) -> Duration {
        match attempt {
            0 => Duration::from_secs(1),
            1 => Duration::from_secs(2),
            2 => Duration::from_secs(4),
            3 => Duration::from_secs(8),
            4 => Duration::from_secs(16),
            _ => Duration::from_secs(30),
        }
    }

    async fn run_connection(
        self: Arc<Self>,
        url: String,
        token: String,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let conn_id = self.connection_id.fetch_add(1, Ordering::SeqCst) + 1;
        info!("WS: Connecting to {} (id={})", url, conn_id);

        let mut request = url.into_client_request()?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|_| "Invalid token")?,
        );

        let (ws_stream, response) =
            match timeout(Duration::from_secs(10), connect_async(request)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(format!("WS connection error: {e}").into()),
                Err(_) => return Err("WS connection timed out after 10s".into()),
            };
        info!(
            "WS: Connected (status={}, id={})",
            response.status(),
            conn_id
        );
        info!("WS: Client connected");

        self.connection_closed.store(false, Ordering::SeqCst);
        self.server_ready.store(false, Ordering::SeqCst);
        self.ping_timestamp.store(0, Ordering::SeqCst);
        self.pong_timestamp.store(0, Ordering::SeqCst);

        let disconnect_notify = Arc::new(Notify::new());
        let (ws_sender, ws_receiver) = ws_stream.split();

        let data_ready = self.data_ready.clone();
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<WsMessage>();

        let self_arc_reader = self.clone();
        let disconnect_for_read = disconnect_notify.clone();
        tokio::spawn(async move {
            let mut ws_receiver = ws_receiver;
            let mut recv_count = 0u64;
            while let Some(msg) = ws_receiver.next().await {
                if self_arc_reader.connection_id.load(Ordering::SeqCst) != conn_id {
                    return;
                }
                let client = self_arc_reader.clone();
                recv_count += 1;
                match msg {
                    Ok(WsMessage::Binary(data)) => {
                        let data_len = data.len();
                        trace!(
                            "WS: Received binary message (size: {} bytes, total received: {}, id={})",
                            data_len,
                            recv_count,
                            conn_id
                        );
                        let message = Message::from_data(data.to_vec());
                        trace!(
                            "WS: Parsed message - id: {}, source: {}, priority: {:?}",
                            message.id,
                            message.source,
                            message.priority
                        );
                        client.handle_message(conn_id, message);
                    }
                    Ok(WsMessage::Text(text)) => {
                        let text_len = text.len();
                        trace!(
                            "WS: Received text message (size: {} bytes, total received: {}, id={})",
                            text_len,
                            recv_count,
                            conn_id
                        );
                        let message = Message::from_data(text.as_bytes().to_vec());
                        trace!(
                            "WS: Parsed message - id: {}, source: {}, priority: {:?}",
                            message.id,
                            message.source,
                            message.priority
                        );
                        client.handle_message(conn_id, message);
                    }
                    Ok(WsMessage::Ping(_)) => {
                        TungsteniteWebsocketClient::handle_ping();
                    }
                    Ok(WsMessage::Pong(_)) => {
                        client.handle_pong(conn_id);
                    }
                    Ok(WsMessage::Close(_)) => {
                        debug!("WS: Connection closed (id={})", conn_id);
                        client.handle_disconnect(conn_id, &disconnect_for_read);
                        break;
                    }
                    Err(e) => {
                        error!("WS: Error receiving (id={}): {}", conn_id, e);
                        client.handle_disconnect(conn_id, &disconnect_for_read);
                        break;
                    }
                    _ => {
                        trace!("WS: Received other message type");
                    }
                }
            }
            self_arc_reader.handle_disconnect(conn_id, &disconnect_for_read);
        });

        // Writer task — owns the SplitSink, drains the writer channel, flushes periodically
        let self_arc_writer = self.clone();
        let disconnect_for_writer = disconnect_notify.clone();
        tokio::spawn(async move {
            let mut writer = ws_sender;
            let mut flush_interval =
                tokio::time::interval(Duration::from_secs(WRITER_FLUSH_INTERVAL_SECONDS));
            let mut send_count = 0u64;
            loop {
                if self_arc_writer.connection_id.load(Ordering::SeqCst) != conn_id {
                    return;
                }
                if self_arc_writer.connection_closed.load(Ordering::SeqCst) {
                    debug!("WS: Writer - connection closed, exiting (id={})", conn_id);
                    return;
                }
                tokio::select! {
                    msg = writer_rx.recv() => {
                        let Some(msg) = msg else {
                            debug!("WS: Writer channel closed (id={})", conn_id);
                            break;
                        };
                        let msg_size = match &msg {
                            WsMessage::Binary(data) => data.len(),
                            WsMessage::Text(text) => text.len(),
                            _ => 0,
                        };
                        trace!("WS: Writer sending message (size: {} bytes, total sent: {}, id={})", msg_size, send_count, conn_id);
                        send_count += 1;
                        let send_start = std::time::Instant::now();
                        trace!("WS: Writer - about to send message (size: {} bytes, id={})", msg_size, conn_id);
                        if let Err(e) = writer.send(msg).await {
                            error!("WS: Writer send error after {:?} (id={}): {}", send_start.elapsed(), conn_id, e);
                            self_arc_writer.handle_disconnect(conn_id, &disconnect_for_writer);
                            break;
                        }
                        trace!("WS: Writer - send completed in {:?} (id={})", send_start.elapsed(), conn_id);
                    }
                    _ = flush_interval.tick() => {
                        trace!("WS: Writer - periodic flush (send count: {}, id={})", send_count, conn_id);
                        let flush_start = std::time::Instant::now();
                        if let Err(e) = writer.flush().await {
                            error!("WS: Writer flush error after {:?} (id={}): {}", flush_start.elapsed(), conn_id, e);
                            self_arc_writer.handle_disconnect(conn_id, &disconnect_for_writer);
                            break;
                        }
                        trace!("WS: Writer - flush completed in {:?} (id={})", flush_start.elapsed(), conn_id);
                    }
                }
            }
        });

        let self_arc_scheduler = self.clone();
        let writer_tx_for_scheduler = writer_tx.clone();
        tokio::spawn(async move {
            loop {
                data_ready.notified().await;

                let client = self_arc_scheduler.clone();
                if client.connection_id.load(Ordering::SeqCst) != conn_id {
                    return;
                }
                if !client.is_server_ready() {
                    trace!(
                        "WS: Scheduler - server not ready, skipping (id={})",
                        conn_id
                    );
                    continue;
                }
                if client.connection_closed.load(Ordering::SeqCst) {
                    debug!(
                        "WS: Scheduler - connection closed, exiting (id={})",
                        conn_id
                    );
                    return;
                }

                let cycle_start = std::time::Instant::now();
                trace!(
                    "WS: Scheduler - starting priority scan cycle (id={})",
                    conn_id
                );

                'reset: loop {
                    let mut had_any_data = false;
                    for p in 0..20 {
                        let mut consecutive_count = 0u32;
                        loop {
                            if client.connection_id.load(Ordering::SeqCst) != conn_id {
                                return;
                            }
                            if consecutive_count >= 16 {
                                trace!(
                                    "WS: Scheduler - reached consecutive cap (16) at priority {} (id={})",
                                    p,
                                    conn_id
                                );
                                break;
                            }

                            let item_to_send = {
                                let mut map = client.queue[p].lock();
                                let mut found = None;
                                for (source, q) in map.iter_mut() {
                                    if let Some(item) = q.pop_front() {
                                        found = Some((item, source.clone()));
                                        break;
                                    }
                                }
                                found
                            };

                            if let Some((item, source)) = item_to_send {
                                had_any_data = true;
                                consecutive_count += 1;
                                let data_len = item.data.len();
                                trace!("WS: Scheduler - processing item from source '{}' at priority {} - size: {} bytes, consecutive: {}, id={}", source, p, data_len, consecutive_count, conn_id);
                                trace!("WS: Scheduler sending message at priority {} - size: {} bytes, consecutive: {}, id={}", p, data_len, consecutive_count, conn_id);
                                if writer_tx_for_scheduler
                                    .send(WsMessage::Binary(item.data.into()))
                                    .is_err()
                                {
                                    error!(
                                        "WS: Scheduler - failed to send to writer channel (id={})",
                                        conn_id
                                    );
                                    return;
                                }
                                trace!(
                                    "WS: Scheduler - message sent to writer channel (id={})",
                                    conn_id
                                );
                            } else {
                                break;
                            }

                            if client.does_higher_priority_data_exist(p) {
                                trace!("WS: Scheduler - higher priority data exists, resetting to priority 0 (id={})", conn_id);
                                continue 'reset;
                            }
                        }
                    }

                    if !had_any_data {
                        break 'reset;
                    }
                }
                trace!(
                    "WS: Scheduler - completed priority scan cycle in {:?} (id={})",
                    cycle_start.elapsed(),
                    conn_id
                );
            }
        });

        let self_arc_ping = self.clone();
        let writer_tx_for_ping = writer_tx.clone();
        let disconnect_for_ping = disconnect_notify.clone();
        tokio::spawn(async move {
            let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECONDS));
            loop {
                ping_interval.tick().await;

                let client = self_arc_ping.clone();
                if client.connection_id.load(Ordering::SeqCst) != conn_id {
                    return;
                }
                if client.connection_closed.load(Ordering::SeqCst) {
                    return;
                }

                if let Err(e) = client.check_pings_internal() {
                    error!("WS: {} (id={})", e, conn_id);
                    error!(
                        "WS: Connection health check failed - aborting (id={})",
                        conn_id
                    );
                    client.handle_disconnect(conn_id, &disconnect_for_ping);
                    return;
                }

                // Reset pong timestamp before sending (matches C++ checkPings)
                client.pong_timestamp.store(0, Ordering::SeqCst);
                let now = TungsteniteWebsocketClient::get_epoch_millis();
                client.ping_timestamp.store(now, Ordering::SeqCst);
                debug!("WS: Sending ping at {} (id={})", now, conn_id);
                if writer_tx_for_ping
                    .send(WsMessage::Ping(Bytes::new()))
                    .is_err()
                {
                    error!(
                        "WS: Failed to send ping - writer channel closed (id={})",
                        conn_id
                    );
                    client.handle_disconnect(conn_id, &disconnect_for_ping);
                    return;
                }
            }
        });

        let self_arc_prune = self.clone();
        tokio::spawn(async move {
            let mut prune_interval = interval(Duration::from_secs(QUEUE_SOURCE_PRUNE_SECONDS));
            loop {
                prune_interval.tick().await;
                let client = self_arc_prune.clone();
                if client.connection_id.load(Ordering::SeqCst) != conn_id {
                    return;
                }
                if client.connection_closed.load(Ordering::SeqCst) {
                    return;
                }
                client.prune_sources();
            }
        });

        disconnect_notify.notified().await;
        Ok(())
    }

    async fn check_for_updates_on_reconnect(self: &Arc<Self>) {
        if self.reconnectable.load(Ordering::SeqCst) {
            let now = Self::get_epoch_millis();
            let last = self.last_update_check.load(Ordering::SeqCst);
            if now.saturating_sub(last) >= 300_000 {
                self.last_update_check.store(now, Ordering::SeqCst);
                let _ = tokio::task::spawn_blocking(move || {
                    crate::update_check::check_for_updates();
                })
                .await;
            }
        }
    }

    async fn supervise_connections(self: Arc<Self>) {
        let mut attempt = 0u32;

        loop {
            let Some(config) = self.load_connection_config() else {
                error!("WS: Missing connection configuration");
                self.connection_closed.store(true, Ordering::SeqCst);
                get_shutdown_notify().notify_waiters();
                return;
            };

            match self
                .clone()
                .run_connection(config.url.clone(), config.token.clone())
                .await
            {
                Ok(()) => {
                    if !self.reconnectable.load(Ordering::SeqCst) {
                        return;
                    }

                    attempt = 0;
                    let delay = Self::reconnect_delay(attempt);
                    warn!("WS: Reconnecting after {:?}", delay);
                    attempt = attempt.saturating_add(1);
                    self.check_for_updates_on_reconnect().await;
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    self.connection_closed.store(true, Ordering::SeqCst);
                    self.server_ready.store(false, Ordering::SeqCst);

                    if !self.reconnectable.load(Ordering::SeqCst) {
                        error!("WS: Failed to connect: {}", e);
                        get_shutdown_notify().notify_waiters();
                        return;
                    }

                    let delay = Self::reconnect_delay(attempt);
                    warn!(
                        "WS: Connection attempt failed: {}. Retrying after {:?}",
                        e, delay
                    );
                    attempt = attempt.saturating_add(1);
                    self.check_for_updates_on_reconnect().await;
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    fn handle_message(self: &Arc<Self>, connection_id: u64, mut message: Message) {
        use crate::messaging::{
            CANCEL_JOB, DELETE_JOB, FILE_DOWNLOAD, FILE_LIST, SUBMIT_JOB, UPLOAD_FILE,
        };
        if self.connection_id.load(Ordering::SeqCst) != connection_id {
            return;
        }
        debug!(
            "WS: Handling message (id={}) - id: {}, source: {}",
            connection_id, message.id, message.source
        );
        match message.id {
            SERVER_READY => {
                info!("WS: Server ready");
                self.server_ready.store(true, Ordering::SeqCst);
                self.data_ready.notify_one();
                get_reconnect_notify().notify_waiters();
            }
            DB_RESPONSE => {
                let db_request_id = message.pop_uint();
                debug!("WS: DB response received - request_id: {}", db_request_id);
                let mut promises = self.db_request_promises.write();
                if let Some(tx) = promises.remove(&db_request_id) {
                    debug!(
                        "WS: Sending DB response to oneshot for ID {}",
                        db_request_id
                    );
                    if tx.send(message).is_err() {
                        warn!(
                            "WS: Failed to send DB response to oneshot for ID {}",
                            db_request_id
                        );
                    } else {
                        trace!("WS: DB response sent successfully");
                    }
                } else {
                    warn!(
                        "WS: Got unexpected DB Request ID response {}",
                        db_request_id
                    );
                }
            }
            SUBMIT_JOB => {
                debug!("WS: Dispatching SUBMIT_JOB message");
                crate::jobs::handle_job_submit(message);
            }
            CANCEL_JOB => {
                debug!("WS: Dispatching CANCEL_JOB message");
                crate::jobs::handle_job_cancel(message);
            }
            DELETE_JOB => {
                debug!("WS: Dispatching DELETE_JOB message");
                crate::jobs::handle_job_delete(message);
            }
            FILE_DOWNLOAD => {
                debug!("WS: Dispatching FILE_DOWNLOAD message");
                crate::files::handle_file_download(message);
            }
            UPLOAD_FILE => {
                debug!("WS: Dispatching UPLOAD_FILE message");
                crate::files::handle_file_upload(message);
            }
            FILE_LIST => {
                debug!("WS: Dispatching FILE_LIST message");
                crate::files::handle_file_list(message);
            }
            _ => {
                warn!("WS: Received unknown message ID {}", message.id);
            }
        }
    }
}

impl WebsocketClient for TungsteniteWebsocketClient {
    fn start(&self, url: String) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>> {
        Box::pin(async move {
            let _ = url;
            Err("start() without an explicit token is no longer supported".into())
        })
    }

    fn start_with_token(
        &self,
        url: String,
        token: String,
        reconnectable: bool,
    ) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>> {
        let self_arc = get_tungstenite_client();
        Box::pin(async move {
            self_arc
                .reconnectable
                .store(reconnectable, Ordering::SeqCst);
            self_arc.connection_closed.store(false, Ordering::SeqCst);
            self_arc.server_ready.store(false, Ordering::SeqCst);
            *self_arc.connection_config.write() = Some(ConnectionConfig { url, token });

            if !self_arc.supervisor_started.swap(true, Ordering::SeqCst) {
                let supervisor = self_arc.clone();
                tokio::spawn(async move {
                    supervisor.supervise_connections().await;
                });
            }

            Ok(())
        })
    }

    fn queue_message(&self, source: String, data: Vec<u8>, priority: Priority) {
        if self.connection_closed.load(Ordering::SeqCst) {
            debug!(
                "WS: Dropping message to source '{}' - connection closed",
                source
            );
            return;
        }

        let p = priority as usize;
        if p >= 20 {
            error!("WS: Invalid priority {}", p);
            return;
        }

        {
            let mut map = self.queue[p].lock();
            let q = map.entry(source.clone()).or_default();
            let data_len = data.len();
            q.push_back(SDataItem { data });
            let queue_len = q.len();
            debug!("WS: Queued message to source '{}' at priority {} - data size: {} bytes, queue depth: {}", source, p, data_len, queue_len);
        }

        self.data_ready.notify_one();
    }

    fn send_db_request(
        &self,
        message: Message,
    ) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>> {
        if self.connection_closed.load(Ordering::SeqCst)
            || !self.server_ready.load(Ordering::SeqCst)
        {
            warn!(
                "send_db_request: WebSocket is disconnected - connection_closed={}, server_ready={}",
                self.connection_closed.load(Ordering::SeqCst),
                self.server_ready.load(Ordering::SeqCst)
            );
            return Box::pin(async { Err("WebSocket is disconnected".into()) });
        }

        let request_id = self.db_request_counter.fetch_add(1, Ordering::SeqCst);
        debug!(
            "send_db_request: created request_id={}, msg_id={}, source={}",
            request_id, message.id, message.source
        );

        let (tx, rx) = oneshot::channel();
        {
            let mut promises = self.db_request_promises.write();
            promises.insert(request_id, tx);
            trace!(
                "send_db_request: inserted promise for request_id={}",
                request_id
            );
        }

        let mut wrapped = Message::new(message.id, message.priority, &message.source);
        wrapped.push_uint(request_id);
        let payload_bytes = message.clone_payload_bytes();
        debug!(
            "send_db_request: wrapping message - payload size: {} bytes",
            payload_bytes.len()
        );
        wrapped.append_raw_bytes(&payload_bytes);

        let data_len = wrapped.get_data().len();
        debug!(
            "send_db_request: queueing wrapped message ({} bytes) to 'db' source",
            data_len
        );
        self.queue_message(
            "db".to_string(),
            wrapped.get_data().clone(),
            Priority::Highest,
        );
        debug!("send_db_request: message queued, waiting for response...");

        Box::pin(async move {
            trace!(
                "send_db_request: awaiting response for request_id={}",
                request_id
            );
            let wait_start = std::time::Instant::now();
            let response = rx.await.map_err(|e| {
                error!(
                    "send_db_request: oneshot channel error after {:?} for request_id={}: {}",
                    wait_start.elapsed(),
                    request_id,
                    e
                );
                Box::new(e) as Box<dyn Error + Send + Sync>
            })?;
            debug!(
                "send_db_request: received response after {:?} for request_id={}",
                wait_start.elapsed(),
                request_id
            );
            Ok(response)
        })
    }

    fn is_server_ready(&self) -> bool {
        self.server_ready.load(Ordering::SeqCst)
    }

    fn is_connection_closed(&self) -> bool {
        self.connection_closed.load(Ordering::SeqCst)
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
static RECONNECT_NOTIFY: std::sync::LazyLock<Arc<Notify>> =
    std::sync::LazyLock::new(|| Arc::new(Notify::new()));
static SHUTDOWN_NOTIFY: std::sync::LazyLock<Arc<Notify>> =
    std::sync::LazyLock::new(|| Arc::new(Notify::new()));

pub(crate) fn get_tungstenite_client() -> Arc<TungsteniteWebsocketClient> {
    let mut client = TUNGSTENITE_CLIENT.write();
    if let Some(ref c) = *client {
        return c.clone();
    }

    let c = TungsteniteWebsocketClient::new();
    *client = Some(c.clone());
    drop(client);
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

pub fn get_reconnect_notify() -> Arc<Notify> {
    RECONNECT_NOTIFY.clone()
}

pub fn get_shutdown_notify() -> Arc<Notify> {
    SHUTDOWN_NOTIFY.clone()
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
    let mut tc = TUNGSTENITE_CLIENT.write();
    *tc = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::DB_JOB_GET_RUNNING_JOBS;
    use crate::tests::fixtures::websocket_server_fixture::WebsocketServerFixture;

    // ============================================================================
    // WebSocket Authentication Tests - ported from websocket_auth_tests.rs
    // ============================================================================

    #[test]
    fn get_reconnect_notify_returns_stable_singleton() {
        let first = get_reconnect_notify();
        let second = get_reconnect_notify();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn get_shutdown_notify_returns_stable_singleton() {
        let first = get_shutdown_notify();
        let second = get_shutdown_notify();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn reconnect_and_shutdown_notifies_are_distinct() {
        let reconnect = get_reconnect_notify();
        let shutdown = get_shutdown_notify();
        assert!(!Arc::ptr_eq(&reconnect, &shutdown));
    }

    #[test]
    fn test_websocket_client_creation() {
        let client = TungsteniteWebsocketClient::new();
        assert!(!client.is_server_ready());
    }

    #[test]
    fn test_start_requires_token() {
        let client = TungsteniteWebsocketClient::new();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(async { client.start("ws://localhost:8001/ws/".to_string()).await });

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no longer supported"));
    }

    #[test]
    #[serial_test::serial]
    fn test_start_with_token() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            client
                .start_with_token(
                    "ws://localhost:8001/ws/".to_string(),
                    "test_token_123".to_string(),
                    false,
                )
                .await
        });

        assert!(result.is_ok());
        assert!(!client.is_connection_closed());
    }

    #[test]
    #[serial_test::serial]
    fn test_start_with_token_marks_reconnectable_flag() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            client
                .start_with_token(
                    "ws://localhost:8001/ws/".to_string(),
                    "test_token_123".to_string(),
                    true,
                )
                .await
        });

        assert!(result.is_ok());
        assert!(client.reconnectable.load(Ordering::SeqCst));
    }

    #[test]
    #[serial_test::serial]
    fn test_send_db_request_prefixes_u32_request_id_before_payload() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();
        client.connection_closed.store(false, Ordering::SeqCst);
        client.server_ready.store(true, Ordering::SeqCst);

        let mut msg = Message::new(DB_JOB_GET_RUNNING_JOBS, Priority::Highest, "database");
        msg.push_ulong(42);

        let fut = client.send_db_request(msg);
        std::mem::drop(fut);

        let queue = client.queue[Priority::Highest as usize].lock();
        let queued = queue.get("db").unwrap().front().unwrap();
        let mut parsed = Message::from_data(queued.data.clone());
        assert_eq!(parsed.id, DB_JOB_GET_RUNNING_JOBS);
        assert_eq!(parsed.pop_uint(), 0);
        assert_eq!(parsed.pop_ulong(), 42);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_handle_db_response_uses_u32_request_id() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();
        let mut server = WebsocketServerFixture::new().await;

        client
            .start_with_token(server.get_url(), "test-token".to_string(), false)
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            while !client.is_server_ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        let mut request = Message::new(DB_JOB_GET_RUNNING_JOBS, Priority::Highest, "database");
        request.push_ulong(42);
        let response_fut = client.send_db_request(request);

        let mut outbound = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outbound.id, DB_JOB_GET_RUNNING_JOBS);
        let request_id = outbound.pop_uint();
        assert_eq!(request_id, 0);
        assert_eq!(outbound.pop_ulong(), 42);

        let mut response = Message::new(DB_RESPONSE, Priority::Highest, "database");
        response.push_uint(request_id);
        response.push_uint(1);
        server.msg_tx.send(response.get_data().clone()).unwrap();

        let mut returned = tokio::time::timeout(Duration::from_secs(2), response_fut)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(returned.pop_uint(), 1);

        server.stop().await;
    }

    // ============================================================================
    // WebSocket Arc Sharing Tests
    // ============================================================================

    #[test]
    fn test_arc_shares_queue_with_original() {
        let client = TungsteniteWebsocketClient::new();
        let test_data = vec![1u8, 2, 3, 4];
        client.queue_message(
            "test_source".to_string(),
            test_data.clone(),
            Priority::Highest,
        );

        let queue = client.queue[Priority::Highest as usize].lock();
        let queued = queue.get("test_source");

        assert!(
            queued.is_some(),
            "Arc should share the same queue as original"
        );
        let q = queued.unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q.front().unwrap().data, test_data);
    }

    #[test]
    fn test_arc_shares_data_ready_notify() {
        let client = TungsteniteWebsocketClient::new();

        let notify_addr = Arc::as_ptr(&client.data_ready);

        assert_eq!(
            notify_addr,
            Arc::as_ptr(&client.data_ready),
            "Arc should share the same data_ready Notify instance"
        );
    }

    #[test]
    fn test_arc_shares_db_request_promises() {
        let client = TungsteniteWebsocketClient::new();

        let (tx, _rx) = tokio::sync::oneshot::channel();
        {
            let mut promises = client.db_request_promises.write();
            promises.insert(42, tx);
        }

        let promises = client.db_request_promises.read();
        assert!(
            promises.contains_key(&42),
            "Arc should share the same db_request_promises map"
        );
    }

    #[test]
    fn test_arc_scheduler_can_process_queued_messages() {
        let client = TungsteniteWebsocketClient::new();
        let test_data = vec![10u8, 20, 30, 40, 50];
        client.queue_message("db".to_string(), test_data.clone(), Priority::Highest);

        let mut queue = client.queue[Priority::Highest as usize].lock();
        let queued_item = queue
            .get_mut("db")
            .and_then(std::collections::VecDeque::pop_front);

        assert!(
            queued_item.is_some(),
            "Arc scheduler should be able to access messages queued by main thread"
        );
        assert_eq!(queued_item.unwrap().data, test_data);
    }

    #[test]
    fn test_arc_shares_connection_config() {
        let client = TungsteniteWebsocketClient::new();

        {
            let mut config = client.connection_config.write();
            *config = Some(ConnectionConfig {
                url: "ws://test:8080".to_string(),
                token: "test-token".to_string(),
            });
        }

        let config = client.connection_config.read();

        assert!(config.is_some(), "Arc should share connection_config");
        assert_eq!(config.as_ref().unwrap().url, "ws://test:8080");
    }

    #[test]
    fn test_reconnect_delay_caps_at_30_seconds() {
        assert_eq!(
            TungsteniteWebsocketClient::reconnect_delay(0),
            Duration::from_secs(1)
        );
        assert_eq!(
            TungsteniteWebsocketClient::reconnect_delay(1),
            Duration::from_secs(2)
        );
        assert_eq!(
            TungsteniteWebsocketClient::reconnect_delay(4),
            Duration::from_secs(16)
        );
        assert_eq!(
            TungsteniteWebsocketClient::reconnect_delay(5),
            Duration::from_secs(30)
        );
        assert_eq!(
            TungsteniteWebsocketClient::reconnect_delay(100),
            Duration::from_secs(30)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn test_reconnects_after_server_restart_when_reconnectable() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();
        let mut server = WebsocketServerFixture::new().await;

        client
            .start_with_token(server.get_url(), "test-token".to_string(), true)
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            while !client.is_server_ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        server.stop().await;

        tokio::time::timeout(Duration::from_secs(2), async {
            while !client.is_connection_closed() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        server.start().await;

        tokio::time::timeout(Duration::from_secs(3), async {
            while client.is_connection_closed() || !client.is_server_ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn test_notifies_shutdown_when_disconnected_without_ltk() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();
        let mut server = WebsocketServerFixture::new().await;
        let shutdown_notify = get_shutdown_notify();
        let shutdown_wait = shutdown_notify.notified();

        client
            .start_with_token(server.get_url(), "test-token".to_string(), false)
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            while !client.is_server_ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        server.stop().await;

        tokio::time::timeout(Duration::from_secs(2), shutdown_wait)
            .await
            .unwrap();
    }

    #[test]
    fn test_queue_message_is_dropped_while_disconnected() {
        let client = TungsteniteWebsocketClient::new();
        client.connection_closed.store(true, Ordering::SeqCst);

        client.queue_message("source".to_string(), vec![1, 2, 3], Priority::Medium);

        let queued: usize = client
            .queue
            .iter()
            .map(|entry| entry.lock().values().map(VecDeque::len).sum::<usize>())
            .sum();
        assert_eq!(queued, 0);
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
    fn test_check_pings_initial_state_succeeds() {
        let client = TungsteniteWebsocketClient::new();

        assert_eq!(client.get_ping_timestamp(), 0);
        assert_eq!(client.get_pong_timestamp(), 0);
        assert!(
            client.check_pings().is_ok(),
            "check_pings should succeed before any ping has been sent"
        );
    }

    #[test]
    fn test_handle_pong_ignores_stale_connection_id() {
        let client = TungsteniteWebsocketClient::new();
        client.connection_id.store(2, Ordering::SeqCst);
        client.ping_timestamp.store(1000, Ordering::SeqCst);

        client.handle_pong(1);

        assert_eq!(
            client.get_pong_timestamp(),
            0,
            "pong from a superseded connection must not update pong_timestamp"
        );
    }

    #[test]
    fn test_handle_pong_updates_current_connection() {
        let client = TungsteniteWebsocketClient::new();
        client.connection_id.store(5, Ordering::SeqCst);
        client.ping_timestamp.store(1000, Ordering::SeqCst);

        client.handle_pong(5);

        assert_ne!(
            client.get_pong_timestamp(),
            0,
            "pong for the active connection should update pong_timestamp"
        );
    }

    #[test]
    fn test_prune_sources_removes_empty_queues() {
        let client = TungsteniteWebsocketClient::new();

        // Add data to queue
        client.queue_message("source1".to_string(), vec![1, 2, 3], Priority::Lowest);
        client.queue_message("source2".to_string(), vec![4, 5, 6], Priority::Lowest);

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
        client.queue_message("source1".to_string(), vec![1, 2, 3], Priority::Lowest);
        client.queue_message("source2".to_string(), vec![4, 5, 6], Priority::Lowest);

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
        client.queue_message("source1".to_string(), vec![1, 2, 3], Priority::Lowest);
        client.queue_message("source2".to_string(), vec![4, 5, 6], Priority::Lowest);

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
        client.queue_message("s1".to_string(), vec![1], Priority::Highest);
        client.queue_message("s2".to_string(), vec![2], Priority::Medium);
        client.queue_message("s3".to_string(), vec![3], Priority::Lowest);

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
    fn test_clear_queued_messages_skips_contended_priorities() {
        let client = TungsteniteWebsocketClient::new();

        client.queue_message("p0".to_string(), vec![1], Priority::Highest);
        client.queue_message("p19".to_string(), vec![1], Priority::Lowest);

        // Manually populate priorities 1, 2, 5 (no named Priority variant).
        {
            let mut map = client.queue[1].lock();
            map.insert(
                "p1".to_string(),
                VecDeque::from(vec![
                    SDataItem { data: vec![1] },
                    SDataItem { data: vec![2] },
                ]),
            );
        }
        {
            let mut map = client.queue[2].lock();
            map.insert(
                "p2".to_string(),
                VecDeque::from(vec![SDataItem { data: vec![1] }]),
            );
        }
        let p5_count: usize = 3;
        {
            let mut map = client.queue[5].lock();
            map.insert(
                "p5".to_string(),
                VecDeque::from(vec![
                    SDataItem { data: vec![1] },
                    SDataItem { data: vec![2] },
                    SDataItem { data: vec![3] },
                ]),
            );
        }

        // Hold a guard on priority 5 to simulate contention.
        let held_guard = client.queue[5].lock();

        let cleared = client.clear_queued_messages();
        // p0 (1) + p1 (2) + p2 (1) + p19 (1) = 5; p5 is contended and skipped.
        assert_eq!(cleared, 5, "should clear all non-contended priorities");

        // Release the guard so we can inspect priority 5's preserved state.
        drop(held_guard);

        for p in [0usize, 1, 2, 19] {
            let map = client.queue[p].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(count, 0, "Priority {p} should be cleared");
            assert!(map.is_empty(), "Priority {p} queue map should be empty");
        }

        {
            let map = client.queue[5].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(
                count, p5_count,
                "Priority 5 should retain its data (skipped during clear)"
            );
        }

        let cleared_again = client.clear_queued_messages();
        assert_eq!(
            cleared_again, p5_count,
            "should clear priority 5 on second pass"
        );

        for p in 0..20 {
            let map = client.queue[p].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(count, 0, "Priority {p} should be cleared after second pass");
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

    #[test]
    fn test_scheduler_starvation_cap_prevents_livelock() {
        let client = TungsteniteWebsocketClient::new();

        // Fill priority 0 (highest) with 20 items
        for i in 0..20 {
            client.queue_message(format!("s{i}"), vec![i as u8], Priority::Highest);
        }

        // Add 1 item at priority 10 (medium)
        client.queue_message("med".to_string(), vec![99], Priority::Medium);

        // Priority 0 should have 20 items
        {
            let map = client.queue[0].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(count, 20, "priority 0 should have 20 items");
        }

        // Priority 10 should have 1 item
        {
            let map = client.queue[10].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(count, 1, "priority 10 should have 1 item");
        }

        // Simulate the scheduler with starvation cap: consume 16 items from p0
        for _ in 0..16 {
            let item = {
                let mut map = client.queue[0].lock();
                let mut found = None;
                for q in map.values_mut() {
                    if let Some(item) = q.pop_front() {
                        found = Some(item);
                        break;
                    }
                }
                found
            };
            assert!(item.is_some(), "should have item at priority 0");
        }

        // After 16 items, p10 item should still be present (not starved)
        {
            let map = client.queue[10].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(count, 1, "priority 10 should not be starved");
        }

        // Priority 0 should have 4 remaining items
        {
            let map = client.queue[0].lock();
            let count: usize = map.values().map(VecDeque::len).sum();
            assert_eq!(count, 4, "priority 0 should have 4 remaining");
        }
    }

    #[test]
    fn test_scheduler_does_higher_priority_data() {
        let client = TungsteniteWebsocketClient::new();

        assert!(!client.does_higher_priority_data_exist(0), "no data at all");

        client.queue_message("s".to_string(), vec![1], Priority::Highest);
        assert!(
            !client.does_higher_priority_data_exist(0),
            "no higher than 0"
        );
        assert!(
            client.does_higher_priority_data_exist(1),
            "data exists at 0, higher than 1"
        );
        assert!(
            client.does_higher_priority_data_exist(19),
            "data exists at 0, higher than 19"
        );
    }

    #[test]
    fn test_does_higher_priority_data_exist_skips_contended_priorities() {
        let client = TungsteniteWebsocketClient::new();

        client.queue_message("p0".to_string(), vec![1], Priority::Highest);
        {
            let mut map = client.queue[5].lock();
            map.insert(
                "p5".to_string(),
                VecDeque::from(vec![SDataItem { data: vec![5] }]),
            );
        }
        client.queue_message("p10".to_string(), vec![10], Priority::Medium);
        client.queue_message("p19".to_string(), vec![19], Priority::Lowest);

        let held_p5 = client.queue[5].lock();

        assert!(
            client.does_higher_priority_data_exist(19),
            "pessimistic: contended priority treated as data-exists"
        );
        assert!(
            client.does_higher_priority_data_exist(6),
            "p0 has data, higher than 6"
        );
        assert!(
            !client.does_higher_priority_data_exist(0),
            "max_priority=0 scans no priorities"
        );

        let contended_client = TungsteniteWebsocketClient::new();
        {
            let mut map = contended_client.queue[5].lock();
            map.insert(
                "p5".to_string(),
                VecDeque::from(vec![SDataItem { data: vec![5] }]),
            );
        }
        let held = contended_client.queue[5].lock();
        assert!(
            contended_client.does_higher_priority_data_exist(6),
            "only candidate is contended — must return true pessimistically"
        );
        assert!(
            !contended_client.does_higher_priority_data_exist(5),
            "max_priority=5 excludes p5, no contention reached"
        );

        drop(held);
        drop(held_p5);

        assert!(
            client.does_higher_priority_data_exist(19),
            "data at p0/p5/p10/p19, higher than 19"
        );
        assert!(
            client.does_higher_priority_data_exist(11),
            "data at p0/p5/p10, higher than 11"
        );
        assert!(
            contended_client.does_higher_priority_data_exist(6),
            "guard dropped: p5 data visible, higher than 6"
        );
        assert!(
            !contended_client.does_higher_priority_data_exist(5),
            "guard dropped: p5 excluded from 0..5, no data"
        );
    }

    #[tokio::test]
    async fn test_update_check_on_reconnect_rate_limits() {
        let client = TungsteniteWebsocketClient::new();
        // last_update_check is initialized to 0
        assert_eq!(client.last_update_check.load(Ordering::SeqCst), 0);

        // Set reconnectable to true so it passes the reconnectable check
        client.reconnectable.store(true, Ordering::SeqCst);

        let client_arc = Arc::new(client);

        // Running check_for_updates_on_reconnect when reconnectable is true
        // updates last_update_check to a non-zero value and executes.
        client_arc.check_for_updates_on_reconnect().await;

        let first_ts = client_arc.last_update_check.load(Ordering::SeqCst);
        assert!(
            first_ts > 0,
            "last_update_check should be updated to a non-zero value"
        );

        // Running it again immediately doesn't update the timestamp (rate limits)
        client_arc.check_for_updates_on_reconnect().await;
        let second_ts = client_arc.last_update_check.load(Ordering::SeqCst);
        assert_eq!(
            first_ts, second_ts,
            "timestamp should not be updated on subsequent immediate check"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_reconnect_race_condition_protection() {
        reset_websocket_client_for_test();
        let client = get_tungstenite_client();

        // 1. Manually set up state as if connection 1 is running
        let conn_id_1 = client.connection_id.fetch_add(1, Ordering::SeqCst) + 1;
        client.connection_closed.store(false, Ordering::SeqCst);
        let notify_1 = Arc::new(Notify::new());

        // 2. Start connection 2 (simulated)
        let conn_id_2 = client.connection_id.fetch_add(1, Ordering::SeqCst) + 1;
        client.connection_closed.store(false, Ordering::SeqCst);
        let notify_2 = Arc::new(Notify::new());

        // 3. Late disconnect call from connection 1
        client.handle_disconnect(conn_id_1, &notify_1);

        // Verify that connection_closed is STILL FALSE because the disconnect was for an old ID
        assert!(
            !client.is_connection_closed(),
            "Old disconnect should not close current connection"
        );

        // Verify that notify_2 was NOT notified
        tokio::select! {
            () = notify_2.notified() => panic!("Current connection should not have been notified"),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }

        // 4. Disconnect call from connection 2
        let notified_2 = notify_2.notified();
        client.handle_disconnect(conn_id_2, &notify_2);

        // Verify that it IS CLOSED now
        assert!(client.is_connection_closed());

        // Verify that notify_2 WAS notified
        tokio::select! {
            () = notified_2 => {},
            () = tokio::time::sleep(Duration::from_millis(50)) => panic!("Current connection should have been notified"),
        }
    }
}
