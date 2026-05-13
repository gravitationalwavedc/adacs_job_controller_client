use crate::messaging::Message;
use crate::websocket::get_websocket_client;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

struct DbRequest {
    msg: Message,
    response_tx: oneshot::Sender<Result<Message, String>>,
}

pub struct DbBridge {
    request_tx: mpsc::Sender<DbRequest>,
    queue_depth: Arc<AtomicUsize>,
}

static DB_BRIDGE: OnceLock<DbBridge> = OnceLock::new();

impl DbBridge {
    pub fn start() {
        let (tx, mut rx) = mpsc::channel::<DbRequest>(128);
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let qd = Arc::clone(&queue_depth);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("DbBridge runtime");
            rt.block_on(async {
                while let Some(req) = rx.recv().await {
                    qd.fetch_sub(1, Ordering::SeqCst);
                    let ws_client = get_websocket_client();
                    if ws_client.is_connection_closed() || !ws_client.is_server_ready() {
                        let _ = req.response_tx.send(Err(
                            "DB request error: WebSocket is disconnected".to_string(),
                        ));
                        continue;
                    }

                    let result = tokio::time::timeout(
                        Duration::from_secs(30),
                        ws_client.send_db_request(req.msg),
                    )
                    .await;
                    let result = match result {
                        Ok(Ok(msg)) => Ok(msg),
                        Ok(Err(e)) => Err(format!("DB request error: {e}")),
                        Err(_) => Err("DB request timed out after 30s".to_string()),
                    };
                    let _ = req.response_tx.send(result);
                }
            });
        });
        let _ = DB_BRIDGE.set(DbBridge {
            request_tx: tx,
            queue_depth,
        });
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
        let (tx, rx) = oneshot::channel();
        let depth = self.queue_depth.fetch_add(1, Ordering::SeqCst) + 1;
        if depth > 50 {
            tracing::warn!("DbBridge queue depth: {} pending requests", depth);
        }
        self.request_tx
            .blocking_send(DbRequest {
                msg,
                response_tx: tx,
            })
            .map_err(|_| "DbBridge channel closed — bridge thread may have panicked".to_string())?;
        rx.blocking_recv()
            .map_err(|_| "DbBridge response channel closed".to_string())?
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
}
