use crate::messaging::Message;
use crate::websocket::get_websocket_client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::{debug, error, info, trace, warn};

struct DbRequest {
    msg: Message,
    response_tx: std::sync::mpsc::Sender<Result<Message, String>>,
}

pub struct DbBridge {
    request_tx: std::sync::mpsc::SyncSender<DbRequest>,
    queue_depth: Arc<AtomicUsize>,
}

static DB_BRIDGE: OnceLock<DbBridge> = OnceLock::new();

impl DbBridge {
    pub fn start() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<DbRequest>(128);
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let qd = Arc::clone(&queue_depth);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("DbBridge runtime");
            rt.block_on(async {
                let mut request_count = 0u64;
                loop {
                    if let Ok(req) = rx.recv() {
                        request_count += 1;
                        let current_depth = qd.fetch_sub(1, Ordering::SeqCst) - 1;
                        let msg_id = req.msg.id;
                        let msg_source = req.msg.source.clone();
                        debug!("DbBridge: processing request #{} - msg_id={}, source={}, queue_depth={}", request_count, msg_id, msg_source, current_depth);

                        let ws_client = get_websocket_client();
                        if ws_client.is_connection_closed() || !ws_client.is_server_ready() {
                            warn!("DbBridge: WebSocket disconnected - rejecting request #{} (msg_id={})", request_count, msg_id);
                            let _ = req.response_tx.send(Err(
                                "DB request error: WebSocket is disconnected".to_string(),
                            ));
                            continue;
                        }

                        debug!("DbBridge: sending request #{} via WebSocket", request_count);
                        let send_start = std::time::Instant::now();
                        let result = tokio::time::timeout(
                            Duration::from_secs(30),
                            ws_client.send_db_request(req.msg),
                        )
                        .await;
                        let elapsed = send_start.elapsed();
                        let result = match result {
                            Ok(Ok(msg)) => {
                                debug!("DbBridge: request #{} completed successfully in {:?}", request_count, elapsed);
                                Ok(msg)
                            }
                            Ok(Err(e)) => {
                                error!("DbBridge: request #{} failed after {:?}: {}", request_count, elapsed, e);
                                Err(format!("DB request error: {e}"))
                            }
                            Err(_) => {
                                error!("DbBridge: request #{} timed out after 30s", request_count);
                                Err("DB request timed out after 30s".to_string())
                            }
                        };
                        if let Err(ref e) = result {
                            warn!("DbBridge: sending error response for request #{}: {}", request_count, e);
                        }
                        let _ = req.response_tx.send(result);
                        trace!("DbBridge: response sent for request #{}", request_count);
                    } else {
                        debug!("DbBridge: channel closed, exiting");
                        break;
                    }
                }
            });
        });
        let _ = DB_BRIDGE.set(DbBridge {
            request_tx: tx,
            queue_depth,
        });
        info!("DbBridge: started");
    }

    pub fn global() -> &'static DbBridge {
        DB_BRIDGE
            .get()
            .expect("DbBridge not started — call DbBridge::start() first")
    }

    /// Returns `Some(&DbBridge)` if started, `None` otherwise.
    /// Used by `send_and_wait` to fall back gracefully in test environments.
    pub fn try_get() -> Option<&'static DbBridge> {
        DB_BRIDGE.get()
    }

    pub fn send(&self, msg: Message) -> Result<Message, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let depth = self.queue_depth.fetch_add(1, Ordering::SeqCst) + 1;
        let msg_id = msg.id;
        let source = msg.source.clone();
        debug!(
            "DbBridge: send() called - msg_id={}, source={}, queue_depth={}",
            msg_id, source, depth
        );
        if depth > 50 {
            tracing::warn!("DbBridge queue depth: {} pending requests", depth);
        }
        let send_start = std::time::Instant::now();
        self.request_tx
            .send(DbRequest {
                msg,
                response_tx: tx,
            })
            .map_err(|_| "DbBridge channel closed — bridge thread may have panicked".to_string())?;
        trace!("DbBridge: request sent to channel");
        let result_start = std::time::Instant::now();
        let result = rx
            .recv()
            .map_err(|_| "DbBridge response channel closed".to_string())?;
        let elapsed = send_start.elapsed();
        let wait_time = result_start.elapsed();
        debug!(
            "DbBridge: send() completed - msg_id={}, source={}, total={:?}, recv_wait={:?}",
            msg_id, source, elapsed, wait_time
        );
        if result.is_err() {
            tracing::warn!(
                "DbBridge: request failed for msg_id={} source={} depth={}",
                msg_id,
                source,
                depth
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::{
        Message, Priority, DB_BUNDLE_CREATE_OR_UPDATE_JOB, DB_BUNDLE_GET_JOB_BY_ID, DB_RESPONSE,
    };
    use crate::websocket::{
        reset_websocket_client_for_test, set_websocket_client, MockWebsocketClient,
    };
    use std::sync::Arc;
    use test_fork::test;

    fn make_test_response(success: bool, job_id: u64) -> Message {
        let mut resp = Message::new(DB_RESPONSE, Priority::Medium, "database");
        resp.push_bool(success);
        resp.push_ulong(job_id);
        resp
    }

    #[test]
    fn test_db_bridge_send_and_receive() {
        reset_websocket_client_for_test();
        DbBridge::start();
        let mut mock = MockWebsocketClient::new();
        mock.expect_is_connection_closed().return_const(false);
        mock.expect_is_server_ready().return_const(true);
        mock.expect_send_db_request().times(1).returning(|_| {
            let resp = make_test_response(true, 42);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let msg = Message::new(DB_BUNDLE_CREATE_OR_UPDATE_JOB, Priority::Medium, "test");
        let result = DbBridge::global().send(msg);
        assert!(result.is_ok(), "send should succeed");

        let mut parsed = Message::from_data(result.unwrap().get_data().clone());
        assert!(parsed.pop_bool(), "response should indicate success");
        assert_eq!(parsed.pop_ulong(), 42, "response should carry job_id");
    }

    #[test]
    fn test_db_bridge_returns_error_on_failure_response() {
        reset_websocket_client_for_test();
        DbBridge::start();
        let mut mock = MockWebsocketClient::new();
        mock.expect_is_connection_closed().return_const(false);
        mock.expect_is_server_ready().return_const(true);
        mock.expect_send_db_request().times(1).returning(|_| {
            let resp = make_test_response(false, 0);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let msg = Message::new(DB_BUNDLE_GET_JOB_BY_ID, Priority::Medium, "test");
        let result = DbBridge::global().send(msg);
        assert!(
            result.is_ok(),
            "send should return Ok even for failure response"
        );
        let mut parsed = Message::from_data(result.unwrap().get_data().clone());
        assert!(!parsed.pop_bool(), "response should indicate failure");
    }

    #[test]
    fn test_db_bridge_global_returns_same_instance() {
        DbBridge::start();

        let first = std::ptr::from_ref(DbBridge::global());
        let second = std::ptr::from_ref(DbBridge::global());

        assert_eq!(
            first, second,
            "DbBridge::global() should return the same singleton instance"
        );
    }

    #[test]
    fn test_db_bridge_try_get_returns_none_before_start() {
        assert!(
            DbBridge::try_get().is_none(),
            "bridge should not exist before start"
        );
    }

    #[test]
    fn test_db_bridge_returns_error_while_disconnected() {
        reset_websocket_client_for_test();
        DbBridge::start();

        let mut mock = MockWebsocketClient::new();
        mock.expect_is_connection_closed().return_const(true);
        mock.expect_is_server_ready().return_const(false);
        mock.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock));

        let msg = Message::new(DB_BUNDLE_GET_JOB_BY_ID, Priority::Medium, "test");
        let result = DbBridge::global().send(msg);

        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("WebSocket is disconnected"));
    }

    #[test]
    fn test_db_bridge_send_from_async_context() {
        #[tokio::main(flavor = "current_thread")]
        async fn inner() {
            reset_websocket_client_for_test();
            DbBridge::start();

            let mut mock = MockWebsocketClient::new();
            mock.expect_is_connection_closed().return_const(false);
            mock.expect_is_server_ready().return_const(true);
            mock.expect_send_db_request().times(1).returning(|_| {
                let resp = make_test_response(true, 42);
                Box::pin(async move { Ok(resp) })
            });
            set_websocket_client(Arc::new(mock));

            let msg = Message::new(DB_BUNDLE_CREATE_OR_UPDATE_JOB, Priority::Medium, "test");
            let result = DbBridge::global().send(msg);

            assert!(result.is_ok(), "send should not panic in async context");
        }

        inner();
    }
}
