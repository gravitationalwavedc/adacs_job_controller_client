// ============================================================================
// Job Cancel Tests - ported from test_cancel_job.cpp
// ============================================================================
use crate::bundle_manager::BundleManager;
use crate::db::job;
use crate::jobs::handle_job_cancel;
use crate::messaging::{
    Message, Priority, CANCELLED, CANCEL_JOB, DB_JOBSTATUS_GET_BY_JOB_ID, DB_JOBSTATUS_SAVE,
    DB_JOB_GET_BY_ID, DB_JOB_GET_BY_JOB_ID, DB_JOB_SAVE, DB_RESPONSE, SYSTEM_SOURCE, UPDATE_JOB,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use mockall::predicate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

use crate::db::jobstatus;

/// In-memory DB state for mock WebSocket
struct MockDbState {
    jobs: HashMap<i64, job::Model>,
    statuses: HashMap<i64, jobstatus::Model>,
    next_job_id: i64,
    next_status_id: i64,
}

impl MockDbState {
    fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            statuses: HashMap::new(),
            next_job_id: 1,
            next_status_id: 1,
        }
    }
}

/// Helper to configure mock WS with DB support using existing state
fn with_db_support(
    mut mock_ws: MockWebsocketClient,
    state: &Arc<std::sync::Mutex<MockDbState>>,
) -> MockWebsocketClient {
    let state_clone = state.clone();

    mock_ws.expect_is_server_ready().returning(|| true);
    mock_ws
        .expect_send_db_request()
        .times(..)
        .returning(move |msg| {
            let mut resp = Message::new(DB_RESPONSE, Priority::Medium, "database");
            match msg.id {
                DB_JOB_SAVE => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let id = m.pop_ulong() as i64;
                    let job_id = m.pop_ulong() as i64;
                    let scheduler_id = m.pop_ulong() as i64;
                    let submitting = m.pop_bool();
                    let submitting_count = m.pop_uint() as i32;
                    let bundle_hash = m.pop_string();
                    let working_directory = m.pop_string();
                    let running = m.pop_bool();
                    let deleting = m.pop_bool();
                    let deleted = m.pop_bool();

                    let mut s = state_clone.lock().unwrap();
                    let saved_id = if id > 0 { id } else { s.next_job_id };
                    if id > 0 {
                        s.next_job_id = std::cmp::max(s.next_job_id, id + 1);
                    } else {
                        s.next_job_id += 1;
                    }
                    let saved = job::Model {
                        id: saved_id,
                        job_id: if job_id > 0 { Some(job_id) } else { None },
                        scheduler_id: if scheduler_id > 0 {
                            Some(scheduler_id)
                        } else {
                            None
                        },
                        submitting,
                        submitting_count,
                        bundle_hash,
                        working_directory,
                        running,
                        deleting,
                        deleted,
                    };
                    s.jobs.insert(saved_id, saved.clone());
                    resp.push_bool(true);
                    resp.push_ulong(saved_id as u64);
                }
                id if id == DB_JOB_GET_BY_JOB_ID => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let job_id = m.pop_ulong() as i64;
                    let s = state_clone.lock().unwrap();
                    let found = s.jobs.values().find(|j| j.job_id == Some(job_id));
                    if let Some(job) = found {
                        resp.push_bool(true);
                        resp.push_uint(1);
                        resp.push_ulong(job.id as u64);
                        resp.push_ulong(job.job_id.unwrap_or(0) as u64);
                        resp.push_ulong(job.scheduler_id.unwrap_or(0) as u64);
                        resp.push_bool(job.submitting);
                        resp.push_uint(job.submitting_count as u32);
                        resp.push_string(&job.bundle_hash);
                        resp.push_string(&job.working_directory);
                        resp.push_bool(job.running);
                        resp.push_bool(job.deleting);
                        resp.push_bool(job.deleted);
                    } else {
                        resp.push_bool(true);
                        resp.push_uint(0);
                    }
                }
                id if id == DB_JOB_GET_BY_ID => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let id = m.pop_ulong() as i64;
                    let s = state_clone.lock().unwrap();
                    let found = s.jobs.get(&id);
                    if let Some(job) = found {
                        resp.push_bool(true);
                        resp.push_uint(1);
                        resp.push_ulong(job.id as u64);
                        resp.push_ulong(job.job_id.unwrap_or(0) as u64);
                        resp.push_ulong(job.scheduler_id.unwrap_or(0) as u64);
                        resp.push_bool(job.submitting);
                        resp.push_uint(job.submitting_count as u32);
                        resp.push_string(&job.bundle_hash);
                        resp.push_string(&job.working_directory);
                        resp.push_bool(job.running);
                        resp.push_bool(job.deleting);
                        resp.push_bool(job.deleted);
                    } else {
                        resp.push_bool(true);
                        resp.push_uint(0);
                    }
                }
                id if id == DB_JOBSTATUS_SAVE => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let id = m.pop_ulong() as i64;
                    let what = m.pop_string();
                    let state = m.pop_int();
                    let job_id = m.pop_ulong() as i64;
                    let mut s = state_clone.lock().unwrap();
                    let saved_id = if id > 0 { id } else { s.next_status_id };
                    if id > 0 {
                        s.next_status_id = std::cmp::max(s.next_status_id, id + 1);
                    } else {
                        s.next_status_id += 1;
                    }
                    let status = jobstatus::Model {
                        id: saved_id,
                        job_id,
                        state,
                        what,
                    };
                    s.statuses.insert(saved_id, status);
                    resp.push_bool(true);
                    resp.push_ulong(saved_id as u64);
                }
                id if id == DB_JOBSTATUS_GET_BY_JOB_ID => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let job_id = m.pop_ulong() as i64;
                    let s = state_clone.lock().unwrap();
                    let statuses: Vec<_> = s
                        .statuses
                        .iter()
                        .filter(|(_, st)| st.job_id == job_id)
                        .map(|(_, st)| st.clone())
                        .collect();
                    resp.push_bool(true);
                    resp.push_uint(statuses.len() as u32);
                    for status in statuses {
                        resp.push_ulong(status.id as u64);
                        resp.push_ulong(status.job_id as u64);
                        resp.push_string(&status.what);
                        resp.push_int(status.state);
                    }
                }
                _ => {
                    resp.push_bool(true);
                    resp.push_uint(0);
                }
            }
            Box::pin(async move { Ok(resp) })
        });

    mock_ws
}

fn setup_cancel_test(
    _db_name: &str,
) -> (
    BundleFixture,
    String,
    Arc<std::sync::Mutex<MockDbState>>,
    TempDir,
) {
    // Reset websocket client to prevent mock bleeding between tests
    crate::websocket::reset_websocket_client_for_test();

    crate::tests::init_python_global();

    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let working_dir = TempDir::new().unwrap();

    // Create state first, then set up mock with DB support
    let state = Arc::new(std::sync::Mutex::new(MockDbState::new()));
    let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
    set_websocket_client(Arc::new(mock_ws));

    // Insert job directly into mock DB state
    let job = job::Model {
        id: 1,
        job_id: Some(1234),
        scheduler_id: Some(4321),
        bundle_hash: bundle_hash.clone(),
        working_directory: working_dir.path().to_string_lossy().to_string(),
        running: true,
        submitting: false,
        submitting_count: 0,
        deleting: false,
        deleted: false,
    };
    state.lock().unwrap().jobs.insert(1, job.clone());

    (fixture, bundle_hash, state, working_dir)
}

#[test_fork::test]
fn test_cancel_job_not_exists() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, _working_dir) = setup_cancel_test(&db_name);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that doesn't exist (job_id + 1)
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1235); // Non-existent job ID
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        // First uint is the job_id
        assert_eq!(result_msg.pop_uint(), 1235);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), CANCELLED);
        assert_eq!(result_msg.pop_string(), "Job has been cancelled");
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_not_running() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, _working_dir) = setup_cancel_test(&db_name);

        // Set job as not running in the mock state
        {
            let mut s = state.lock().unwrap();
            if let Some(job) = s.jobs.get_mut(&1) {
                job.running = false;
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that is not running
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        // First uint is the job_id
        assert_eq!(result_msg.pop_uint(), 1234);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), CANCELLED);
        assert_eq!(result_msg.pop_string(), "Job has been cancelled");

        // Archive should not have been generated
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        let archive_path =
            std::path::Path::new(&job_refreshed.working_directory).join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_submitting() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, working_dir) = setup_cancel_test(&db_name);

        // Set job as submitting in the mock state
        {
            let mut s = state.lock().unwrap();
            if let Some(job) = s.jobs.get_mut(&1) {
                job.submitting = true;
            }
        }

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        // No messages should be sent when job is submitting
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that is submitting
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should still be running
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(job_refreshed.running);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, working_dir) = setup_cancel_test(&db_name);

        // Write a cancel script that returns true (success)
        fixture.write_job_cancel(&bundle_hash, "True");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // Cancel the job
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        // First uint is the job_id
        assert_eq!(result_msg.pop_uint(), 1234);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), CANCELLED);
        assert_eq!(result_msg.pop_string(), "Job has been cancelled");

        // Job should no longer be running
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.running);

        // Archive should have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_not_running_after_status_check() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, working_dir) = setup_cancel_test(&db_name);

        // Write a status script that returns complete=true (job finished during cancel attempt)
        let status_json = r#"{"status": [], "complete": true}"#;
        fixture.write_job_status_complete(&bundle_hash, status_json);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that is initially running, but completes during status check
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        assert_eq!(result_msg.pop_uint(), 1234);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        // Should be COMPLETED, not CANCELLED
        assert_eq!(result_msg.pop_uint(), crate::messaging::COMPLETED);
        assert_eq!(result_msg.pop_string(), "Job has completed");

        // Job should no longer be running
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.running);

        // Archive should have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_running_after_status_check_cancel_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, working_dir) = setup_cancel_test(&db_name);

        // Write cancel+status scripts: status returns complete=false, cancel returns False (error)
        let status_json = r#"{"status": [], "complete": false}"#;
        fixture.write_job_cancel_check_status(
            &bundle_hash,
            4321,
            1234,
            "test_cluster",
            status_json,
            "False",
        );

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        // No messages should be sent when cancel fails
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that is running, status check says still running, but cancel fails
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should still be running
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(job_refreshed.running);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_running_after_status_check_cancel_success_already_cancelled() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, working_dir) = setup_cancel_test(&db_name);

        // Write cancel+status scripts: status returns complete=false, cancel returns True
        let status_json = r#"{"status": [], "complete": false}"#;
        fixture.write_job_cancel_check_status(
            &bundle_hash,
            4321,
            1234,
            "test_cluster",
            status_json,
            "True",
        );

        // Set up mock first for DB operations
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(..)
            .returning(|_, _, _, _| {});
        set_websocket_client(Arc::new(mock_ws));

        // Add a CANCELLED status record to simulate already cancelled state
        // Note: job_id here is the internal DB id (1), not the external job_id (1234)
        {
            let mut s = state.lock().unwrap();
            s.statuses.insert(
                1,
                jobstatus::Model {
                    id: 1,
                    job_id: 1,
                    state: crate::messaging::CANCELLED as i32,
                    what: "test".to_string(),
                },
            );
        }

        // Re-create mock with strict expectations for the actual test
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        // No messages should be sent when already cancelled
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that already has CANCELLED status
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);
        tokio::task::yield_now().await;

        // Job should still be running (cancel shouldn't have changed anything)
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(job_refreshed.running);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_cancel_job_running_after_status_check_cancel_success_not_already_cancelled() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, working_dir) = setup_cancel_test(&db_name);

        // Write cancel+status scripts: status returns complete=false, cancel returns True
        let status_json = r#"{"status": [], "complete": false}"#;
        fixture.write_job_cancel_check_status(
            &bundle_hash,
            4321,
            1234,
            "test_cluster",
            status_json,
            "True",
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // Try to cancel a job that is running, status check says still running, cancel succeeds
        let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_cancel(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        assert_eq!(result_msg.pop_uint(), 1234);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), CANCELLED);
        assert_eq!(result_msg.pop_string(), "Job has been cancelled");

        // Job should no longer be running
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.running);

        // Archive should have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(archive_path.exists());
    }
    inner();
}
