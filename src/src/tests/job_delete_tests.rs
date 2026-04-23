// ============================================================================
// Job Delete Tests - ported from test_delete_job.cpp
// ============================================================================

use crate::bundle_manager::BundleManager;
use crate::db::{self, get_db, job};
use crate::jobs::handle_job_delete;
use crate::messaging::Message;
use crate::messaging::{Priority, DELETED, DELETE_JOB, SYSTEM_SOURCE, UPDATE_JOB};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

async fn setup_delete_test(db_name: &str) -> (BundleFixture, String, job::Model, TempDir) {
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
        running: false,
        deleted: false,
        ..Default::default()
    };
    let job = crate::db::save_job(db, job).await.unwrap();

    (fixture, bundle_hash, job, working_dir)
}

#[tokio::test]
async fn test_delete_job_not_exists() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, _job, _working_dir) = setup_delete_test(&db_name).await;

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

    // Use a unique non-existent job ID for this test
    let non_existent_job_id = 99999;

    // Try to delete a job that doesn't exist
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(non_existent_job_id);
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

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

#[tokio::test]
async fn test_delete_job_success() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_delete_test(&db_name).await;

    // Write a delete script that returns true (success)
    fixture.write_job_delete(&bundle_hash, "True");

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

    // Delete the job
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

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
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(job_refreshed.deleted);
    assert!(!job_refreshed.deleting);
}

#[tokio::test]
async fn test_delete_job_job_running() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, mut job, working_dir) = setup_delete_test(&db_name).await;

    // Set job as running
    let db = get_db();
    job.running = true;
    job = db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when job is running
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to delete a job that is running
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Job should not be deleted
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.deleted);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_delete_job_job_submitting() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, mut job, working_dir) = setup_delete_test(&db_name).await;

    // Set job as submitting
    let db = get_db();
    job.submitting = true;
    job = db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when job is submitting
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to delete a job that is submitting
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Job should not be deleted
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.deleted);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_delete_job_failure() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_delete_test(&db_name).await;

    // Write a delete script that returns False (failure)
    fixture.write_job_delete(&bundle_hash, "False");

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when delete fails
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to delete a job where the bundle fails
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Job should not be deleted or deleting after failure
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.deleted);
    assert!(!job_refreshed.deleting);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}

#[tokio::test]
async fn test_delete_job_job_deleted() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, mut job, _working_dir) = setup_delete_test(&db_name).await;

    // Set job as already deleted
    let db = get_db();
    job.deleted = true;
    let _ = db::save_job(db, job).await.unwrap();

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

    // Try to delete a job that is already deleted (use non-existent job ID)
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(99999); // Non-existent job
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

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

#[tokio::test]
async fn test_delete_job_job_deleting() {
    let db_name = Uuid::new_v4().to_string();
    let (_fixture, _bundle_hash, mut job, working_dir) = setup_delete_test(&db_name).await;

    // Set job as already deleting
    let db = get_db();
    job.deleting = true;
    job = db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when job is already deleting
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Try to delete a job that is already deleting
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(1234);
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_delete(msg).await;

    // Wait a moment
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Job should not be deleted
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.deleted);

    // Archive should not have been generated
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(!archive_path.exists());
}
