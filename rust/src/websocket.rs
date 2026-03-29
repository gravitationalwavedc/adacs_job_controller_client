use crate::messaging::{Message, Priority, SERVER_READY, DB_RESPONSE};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::{interval, Duration};
use futures_util::{StreamExt, SinkExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use log::{info, error, warn};
use std::pin::Pin;
use std::future::Future;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type MessageCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[mockall::automock]
pub trait WebsocketClient: Send + Sync {
    fn start(&self, url: String) -> BoxFuture<'static, Result<(), Box<dyn Error + Send + Sync>>>;
    fn queue_message(&self, source: String, data: Vec<u8>, priority: Priority, callback: MessageCallback);
    fn send_db_request(&self, message: Message) -> BoxFuture<'static, Result<Message, Box<dyn Error + Send + Sync>>>;
    fn is_server_ready(&self) -> bool;
}

struct SDataItem {
    data: Vec<u8>,
    callback: MessageCallback,
}

type PriorityQueue = Vec<Mutex<HashMap<String, VecDeque<SDataItem>>>>;

pub struct TungsteniteWebsocketClient {
    server_ready: AtomicBool,
    db_request_counter: AtomicU64,
    db_request_promises: RwLock<HashMap<u64, oneshot::Sender<Message>>>,
    queue: PriorityQueue,
    data_ready: Arc<Notify>,
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
        })
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

            let (mut ws_sender, mut ws_receiver) = ws_stream.split();
            
            let data_ready = client.data_ready.clone();
            let (tx_out, mut rx_out) = mpsc::unbounded_channel::<Vec<u8>>();

            // Task for reading from WS
            let client_for_read = client.clone();
            tokio::spawn(async move {
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
                        Ok(WsMessage::Close(_)) => {
                            info!("WS: Connection closed");
                            break;
                        }
                        Err(e) => {
                            error!("WS: Error receiving: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            });

            // Task for sending to WS
            tokio::spawn(async move {
                while let Some(data) = rx_out.recv().await {
                    if let Err(e) = ws_sender.send(WsMessage::Binary(data.into())).await {
                        error!("WS: Error sending: {}", e);
                        break;
                    }
                }
            });

            // Background scheduler task
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

            // Ping task
            tokio::spawn(async move {
                let mut _interval = interval(Duration::from_secs(30));
                loop {
                    _interval.tick().await;
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
