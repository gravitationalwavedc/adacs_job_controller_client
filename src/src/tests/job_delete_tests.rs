// ============================================================================
// Job Delete Tests - ported from test_delete_job.cpp
// ============================================================================
use crate::bundle_manager::BundleManager;
use crate::db::job;
use crate::jobs::handle_job_delete;
use crate::messaging::{
    Message, Priority, DB_JOB_GET_BY_ID, DB_JOB_GET_BY_JOB_ID, DB_JOB_SAVE, DB_RESPONSE, DELETED,
    DELETE_JOB, SYSTEM_SOURCE, UPDATE_JOB,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use mockall::predicate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

/// In-memory DB state for mock WebSocket
struct MockDbState {
    jobs: HashMap<i64, job::Model>,
    next_job_id: i64,
}

impl MockDbState {
    fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            next_job_id: 1,
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
                _ => {
                    resp.push_bool(true);
                    resp.push_uint(0);
                }
            }
            Box::pin(async move { Ok(resp) })
        });

    mock_ws
}

fn setup_delete_test(
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

    // Create state and mock BEFORE any DB calls
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
        running: false,
        deleted: false,
        submitting: false,
        submitting_count: 0,
        deleting: false,
    };
    state.lock().unwrap().jobs.insert(1, job.clone());

    (fixture, bundle_hash, state, working_dir)
}

#[test_fork::test]
fn test_delete_job_not_exists() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, _working_dir) = setup_delete_test(&db_name);

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

        // Use a unique non-existent job ID for this test
        let non_existent_job_id = 99999;

        // Try to delete a job that doesn't exist
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(non_existent_job_id);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        assert_eq!(result_msg.pop_uint(), non_existent_job_id);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), DELETED);
        assert_eq!(result_msg.pop_string(), "Job has been deleted");
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, _working_dir) = setup_delete_test(&db_name);

        // Write a delete script that returns true (success)
        fixture.write_job_delete(&bundle_hash, "True");

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

        // Delete the job
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        assert_eq!(result_msg.pop_uint(), 1234);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), DELETED);
        assert_eq!(result_msg.pop_string(), "Job has been deleted");

        // Job should be marked as deleted
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(job_refreshed.deleted);
        assert!(!job_refreshed.deleting);
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_job_running() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, working_dir) = setup_delete_test(&db_name);

        // Set job as running in the mock state
        {
            let mut s = state.lock().unwrap();
            if let Some(job) = s.jobs.get_mut(&1) {
                job.running = true;
            }
        }

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        // No messages should be sent when job is running
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Try to delete a job that is running
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should not be deleted
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.deleted);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_job_submitting() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, working_dir) = setup_delete_test(&db_name);

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

        // Try to delete a job that is submitting
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should not be deleted
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.deleted);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, state, working_dir) = setup_delete_test(&db_name);

        // Write a delete script that returns False (failure)
        fixture.write_job_delete(&bundle_hash, "False");

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        // No messages should be sent when delete fails
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Try to delete a job where the bundle fails
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should not be deleted or deleting after failure
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.deleted);
        assert!(!job_refreshed.deleting);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_job_deleted() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, _working_dir) = setup_delete_test(&db_name);

        // Set job as already deleted in the mock state
        {
            let mut s = state.lock().unwrap();
            if let Some(job) = s.jobs.get_mut(&1) {
                job.deleted = true;
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

        // Try to delete a job that is already deleted (use non-existent job ID)
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(99999); // Non-existent job
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait for message
        let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for UPDATE_JOB message")
            .expect("No message received");

        let mut result_msg = Message::from_data(msg_data);
        assert_eq!(result_msg.id, UPDATE_JOB);
        assert_eq!(result_msg.pop_uint(), 99999);
        assert_eq!(result_msg.pop_string(), "_job_completion_");
        assert_eq!(result_msg.pop_uint(), DELETED);
        assert_eq!(result_msg.pop_string(), "Job has been deleted");
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_job_deleting() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (_fixture, _bundle_hash, state, working_dir) = setup_delete_test(&db_name);

        // Set job as already deleting in the mock state
        {
            let mut s = state.lock().unwrap();
            if let Some(job) = s.jobs.get_mut(&1) {
                job.deleting = true;
            }
        }

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        // No messages should be sent when job is already deleting
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Try to delete a job that is already deleting
        let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(1234);
        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_job_delete(msg);

        // Wait a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Job should not be deleted
        let job_refreshed = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(!job_refreshed.deleted);

        // Archive should not have been generated
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(!archive_path.exists());
    }
    inner();
}
