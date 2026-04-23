// ============================================================================
// Job Cancel Tests - ported from test_cancel_job.cpp
// ============================================================================

use crate::bundle_manager::BundleManager;
use crate::db::{self, get_db, job};
use crate::jobs::handle_job_cancel;
use crate::messaging::Message;
use crate::messaging::{Priority, CANCELLED, CANCEL_JOB, SYSTEM_SOURCE, UPDATE_JOB};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

async fn setup_cancel_test(db_name: &str) -> (BundleFixture, String, job::Model, TempDir) {
    // Reset DB for complete isolation using unique in-memory database
    crate::db::reset_for_test(&format!("sqlite:{}?mode=memory&cache=private", db_name))
        .await
        .expect("Failed to reset DB");

    // Reset websocket client to prevent mock bleeding between tests
    crate::websocket::reset_websocket_client_for_test();

    let db = crate::db::get_db();

    crate::tests::init_python_global();

    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let working_dir = TempDir::new().unwrap();

    let job = job::Model {
        id: 0,
        job_id: Some(1234),
        scheduler_id: Some(4321),
        bundle_hash: bundle_hash.clone(),
        working_directory: working_dir.path().to_string_lossy().to_string(),
        running: true,
        ..Default::default()
    };
    let job = crate::db::save_job(db, job).await.unwrap();

    (fixture, bundle_hash, job, working_dir)
}

#[tokio::test]
async fn test_cancel_job_not_exists() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, _job, _working_dir) = setup_cancel_test(&db_name).await;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws
        .expect_queue_message()
        .times(1)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that doesn't exist (job_id + 1)
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1235); // Non-existent job ID
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

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

#[tokio::test]
async fn test_cancel_job_not_running() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, mut job, _working_dir) = setup_cancel_test(&db_name).await;

    // Set job as not running
    let db = get_db();
    job.running = false;
    job = db::save_job(db, job).await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws
        .expect_queue_message()
        .times(1)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that is not running
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

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
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    let archive_path =
        std::path::Path::new(&job_refreshed.working_directory).join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_cancel_job_submitting() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, mut job, working_dir) = setup_cancel_test(&db_name).await;

    // Set job as submitting
    let db = get_db();
    job.submitting = true;
    job = db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when job is submitting
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that is submitting
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Job should still be running
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(job_refreshed.running);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_cancel_job_success() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_cancel_test(&db_name).await;

    // Write a cancel script that returns true (success)
    fixture.write_job_cancel(&bundle_hash, "True");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws
        .expect_queue_message()
        .times(1)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Cancel the job
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

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
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.running);

    // Archive should have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(archive_path.exists());
}

#[tokio::test]
async fn test_cancel_job_not_running_after_status_check() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_cancel_test(&db_name).await;

    // Write a status script that returns complete=true (job finished during cancel attempt)
    let status_json = r#"{"status": [], "complete": true}"#;
    fixture.write_job_status_complete(&bundle_hash, status_json);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws
        .expect_queue_message()
        .times(1)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that is initially running, but completes during status check
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

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
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.running);

    // Archive should have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(archive_path.exists());
}

#[tokio::test]
async fn test_cancel_job_running_after_status_check_cancel_error() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_cancel_test(&db_name).await;

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

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when cancel fails
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that is running, status check says still running, but cancel fails
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Job should still be running
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(job_refreshed.running);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_cancel_job_running_after_status_check_cancel_success_already_cancelled() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_cancel_test(&db_name).await;

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

    // Add a CANCELLED status record to simulate already cancelled state
    let db = get_db();
    let cancelled_status = db::jobstatus::Model {
        id: 0,
        job_id: job.id,
        state: crate::messaging::CANCELLED as i32,
        what: "test".to_string(),
    };
    db::save_status(db, cancelled_status).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when already cancelled
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that already has CANCELLED status
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

    // Job should still be running (cancel shouldn't have changed anything)
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(job_refreshed.running);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_cancel_job_running_after_status_check_cancel_success_not_already_cancelled() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_cancel_test(&db_name).await;

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

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws
        .expect_queue_message()
        .times(1)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to cancel a job that is running, status check says still running, cancel succeeds
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = handle_job_cancel(msg).await;
    handle.await.unwrap();

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
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.running);

    // Archive should have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(archive_path.exists());
}
