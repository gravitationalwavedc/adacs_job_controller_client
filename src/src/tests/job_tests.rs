use crate::bundle_manager::BundleManager;
use crate::db::{self, get_db, get_or_create_by_job_id};
use crate::jobs::{handle_job_submit, MAX_SUBMIT_COUNT};
use crate::messaging::{Message, Priority, ERROR, SUBMIT_JOB, SYSTEM_SOURCE};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use mockall::predicate::*;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

pub async fn setup_test(db_name: &str) {
    crate::tests::init_python_global();

    // Use a unique in-memory database for each test and RESET it to ensure isolation
    db::reset_for_test(&format!("sqlite:{}?mode=memory&cache=private", db_name))
        .await
        .expect("Failed to initialize DB");

    let db = get_db();
    use sea_orm::{ConnectionTrait, DbBackend};

    let schema_job = r#"
        CREATE TABLE jobclient_job (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER,
            scheduler_id INTEGER,
            submitting BOOLEAN NOT NULL,
            submitting_count INTEGER NOT NULL,
            bundle_hash TEXT NOT NULL,
            working_directory TEXT NOT NULL,
            running BOOLEAN NOT NULL,
            deleted BOOLEAN NOT NULL,
            deleting BOOLEAN NOT NULL
        );
    "#;
    let schema_status = r#"
        CREATE TABLE jobclient_jobstatus (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            what TEXT NOT NULL,
            state INTEGER NOT NULL,
            job_id INTEGER NOT NULL,
            FOREIGN KEY(job_id) REFERENCES jobclient_job(id)
        );
    "#;

    let _ = db
        .execute(sea_orm::Statement::from_string(
            DbBackend::Sqlite,
            schema_job.to_string(),
        ))
        .await;
    let _ = db
        .execute(sea_orm::Statement::from_string(
            DbBackend::Sqlite,
            schema_status.to_string(),
        ))
        .await;
}

#[tokio::test]
async fn test_submit_timeout() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let mut mock_ws = MockWebsocketClient::new();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    // We expect 1 message when it finally submits
    mock_ws
        .expect_queue_message()
        .with(always(), always(), eq(Priority::Medium), always())
        .times(1)
        .returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });

    mock_ws.expect_is_server_ready().returning(|| true);

    let ws_arc = Arc::new(mock_ws);
    set_websocket_client(ws_arc);

    let job_id = 1234i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = true;
    let _ = db::save_job(db, job.clone()).await;

    // Try to submit the job many times
    for count in 1..MAX_SUBMIT_COUNT {
        let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&bundle_hash);
        msg_raw.push_string("test params");

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_job_submit(msg).await;

        let mut retry = 0;
        while retry < 10 {
            job = get_or_create_by_job_id(db, job_id).await.unwrap();
            if job.submitting_count == count {
                break;
            }
            sleep(Duration::from_millis(50)).await;
            retry += 1;
        }
        assert_eq!(
            job.submitting_count, count,
            "Client never incremented submittingCount at count {}",
            count
        );
    }

    // Now write a successful submit script
    fixture.write_job_submit(&bundle_hash, "/a/test/working/directory/", "4321");

    let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&bundle_hash);
    msg_raw.push_string("test params");

    let msg = Message::from_data(msg_raw.get_data().clone());
    handle_job_submit(msg).await;

    // Wait for the message to be queued
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timed out waiting for queue_message");

    let mut retry = 0;
    while retry < 20 {
        job = get_or_create_by_job_id(db, job_id).await.unwrap();
        if !job.working_directory.is_empty() {
            break;
        }
        sleep(Duration::from_millis(100)).await;
        retry += 1;
    }

    assert_eq!(job.submitting_count, 0);
    assert_eq!(job.job_id, Some(job_id));
    assert_eq!(job.working_directory, "/a/test/working/directory/");
    assert!(!job.submitting);
    assert_eq!(job.scheduler_id, Some(4321));
}

#[tokio::test]
async fn test_submit_error_none() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_job_submit_error(&bundle_hash, "None");

    let mut mock_ws = MockWebsocketClient::new();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    // We expect 2 messages on error
    mock_ws
        .expect_queue_message()
        .times(2)
        .returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });

    mock_ws.expect_is_server_ready().returning(|| true);

    let ws_arc = Arc::new(mock_ws);
    set_websocket_client(ws_arc);

    let job_id = 5678u32;
    let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id);
    msg_raw.push_string(&bundle_hash);
    msg_raw.push_string("test params");

    let msg = Message::from_data(msg_raw.get_data().clone());
    handle_job_submit(msg).await;

    // Wait for messages to be processed (2 calls)
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timed out waiting for first queue_message");
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timed out waiting for second queue_message");

    let db = get_db();
    let job_result = crate::db::get_job_by_id(db, job_id as i64).await.unwrap();
    // In C++ test, it deletes the job on submit error
    assert!(job_result.is_none() || job_result.unwrap().id == 0);
}

#[tokio::test]
async fn test_submit_error_zero() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    // Write submit script that returns 0 (error case)
    fixture.write_job_submit_error(&bundle_hash, "0");

    let mut mock_ws = MockWebsocketClient::new();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    // We expect 2 messages on error
    mock_ws
        .expect_queue_message()
        .times(2)
        .returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });

    mock_ws.expect_is_server_ready().returning(|| true);

    let ws_arc = Arc::new(mock_ws);
    set_websocket_client(ws_arc);

    let job_id = 1234u32;
    let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id);
    msg_raw.push_string(&bundle_hash);
    msg_raw.push_string("test params");

    let msg = Message::from_data(msg_raw.get_data().clone());
    handle_job_submit(msg).await;

    // Wait for messages to be processed
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timed out waiting for first message");
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timed out waiting for second message");

    // Job should be deleted after submit error
    let db = get_db();
    let job = db::get_job_by_job_id(db, job_id as i64).await.unwrap();
    assert!(job.is_none(), "Job should be deleted after submit error");
}

#[tokio::test]
async fn test_check_status_job_running_same_status() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with RUNNING status (same as will be in DB)
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

    // Create a RUNNING status record
    let db = get_db();
    let status = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "test_what".to_string(),
        state: RUNNING as i32,
    };
    let status = db::save_status(db, status).await.unwrap();
    let original_id = status.id;

    let mut mock_ws = MockWebsocketClient::new();
    // Should NOT send notification when same status and force_notification=false
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Check status with force_notification=false
    check_job_status(job.clone(), false).await;

    // Status record should be updated (same id), not created new
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 1);
    assert_eq!(v_status[0].id, original_id);
    assert_eq!(v_status[0].state, RUNNING as i32);
}

#[tokio::test]
async fn test_check_status_job_running_same_status_multiple() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with multiple status entries, all RUNNING
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Info 1", "what": "what1", "status": 50}, {"info": "Info 2", "what": "what2", "status": 50}], "complete": false}"#);

    // Create two RUNNING status records
    let db = get_db();
    let status1 = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "what1".to_string(),
        state: RUNNING as i32,
    };
    let status1 = db::save_status(db, status1).await.unwrap();
    let original_id1 = status1.id;

    let status2 = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "what2".to_string(),
        state: RUNNING as i32,
    };
    let status2 = db::save_status(db, status2).await.unwrap();
    let original_id2 = status2.id;

    let mut mock_ws = MockWebsocketClient::new();
    // Should NOT send notification when same status and force_notification=false
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Check status with force_notification=false
    check_job_status(job.clone(), false).await;

    // Status records should be updated (same ids), not created new
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 2);
    let ids: Vec<i64> = v_status.iter().map(|s| s.id).collect();
    assert!(ids.contains(&original_id1));
    assert!(ids.contains(&original_id2));
}

#[tokio::test]
async fn test_archive_success() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    let job_id = 999i64;
    let mut job = db::job::Model {
        id: 0,
        job_id: Some(job_id),
        scheduler_id: Some(123),
        submitting: false,
        submitting_count: 0,
        bundle_hash: "hash".to_string(),
        working_directory: working_dir.clone(),
        running: false,
        deleted: false,
        deleting: false,
    };
    let db = get_db();
    job = db::save_job(db, job).await.unwrap();

    let result = crate::jobs::archive_job(&job).await;
    assert!(result);
    assert!(std::path::Path::new(&working_dir)
        .join("archive.tar.gz")
        .exists());
}

#[tokio::test]
async fn test_cancel_job_success() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();

    // Write cancel script that returns true
    fixture.write_job_cancel(&bundle_hash, "True");

    let job_id = 777i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.running = true;
    job.bundle_hash = bundle_hash.clone();
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    mock_ws.expect_queue_message().returning(move |_, _, _, _| {
        let _ = tx_clone.send(());
    });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));

    use crate::messaging::CANCEL_JOB;
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    let msg = Message::from_data(msg_raw.get_data().clone());

    let handle = crate::jobs::handle_job_cancel(msg).await;
    handle.await.unwrap();

    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for cancel msg");

    let job_after = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    assert!(!job_after.running);
}

#[tokio::test]
async fn test_submit_already_submitted() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    // Create a test file for archiving
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Write a status check script that returns complete=true with COMPLETED status
    // This simulates a job that has already finished
    fixture.write_job_submit_check_status(
        &bundle_hash,
        &working_dir,
        "4321",
        r#"{"status": [{"what": "job", "status": 500, "info": "Completed"}], "complete": true}"#,
    );

    let job_id = 1250i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.scheduler_id = Some(4321); // Already has scheduler ID
    job.running = true; // Already running
    job.bundle_hash = bundle_hash.clone();
    job.working_directory = working_dir.clone();
    db::save_job(db, job).await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    // Expect 2 messages: status update + job completion notification from check_job_status
    mock_ws
        .expect_queue_message()
        .times(2)
        .returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    let mut msg_raw = Message::new(
        crate::messaging::SUBMIT_JOB,
        Priority::Medium,
        SYSTEM_SOURCE,
    );
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&bundle_hash);
    msg_raw.push_string(""); // params
    let msg = Message::from_data(msg_raw.get_data().clone());

    handle_job_submit(msg).await;

    // Wait for the messages (status update + completion)
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for first message");
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for completion message");

    // Give async operations time to complete
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Job should complete and archive should be created
    let job_after = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    assert!(!job_after.running, "Job should no longer be running");

    // Verify archive was created
    let archive_path = std::path::Path::new(&working_dir).join("archive.tar.gz");
    assert!(archive_path.exists(), "Archive should be created");
}

#[tokio::test]
async fn test_check_all_job_status_stress() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let db = get_db();
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    // We expect 100 messages
    mock_ws
        .expect_queue_message()
        .times(100)
        .returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));

    // Create 100 jobs
    for i in 0..100 {
        let job_id = (i + 1000) as i64;
        let bundle_hash = format!("bundle_{}", i);
        let temp_dir = tempfile::TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_str().unwrap().to_string();

        fixture.write_job_submit_check_status(
            &bundle_hash,
            &working_dir,
            &format!("{}", job_id),
            r#"{"complete": true}"#,
        );

        let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
        job.job_id = Some(job_id);
        job.running = true;
        job.bundle_hash = bundle_hash;
        job.working_directory = working_dir;
        db::save_job(db, job).await.unwrap();
    }

    crate::jobs::check_all_jobs_status().await;

    for _ in 0..100 {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("Timeout waiting for stress test status update");
    }
}

#[tokio::test]
async fn test_check_status_job_running_new_status() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();

    // Status check returns a new status (key is "status" matching C++ CheckStatus.cpp)
    fixture.write_job_submit_check_status(
        &bundle_hash,
        &working_dir,
        "123",
        r#"{"complete": false, "status": [{"what": "stage1", "status": 50}]}"#,
    );

    let job_id = 111i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.running = true;
    job.bundle_hash = bundle_hash.clone();
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    mock_ws.expect_queue_message().returning(move |_, _, _, _| {
        let _ = tx_clone.send(());
    });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));

    let job_model = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    crate::jobs::check_job_status(job_model.clone(), false).await;

    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for status update");

    // Query by internal PK (job_model.id), not external job_id - status FK references job.id
    let status_after = db::get_job_status_by_job_id(db, job_model.id)
        .await
        .unwrap();
    assert_eq!(status_after.len(), 1);
    assert_eq!(status_after[0].what, "stage1");
    assert_eq!(status_after[0].state, 50); // RUNNING = 50
}

#[tokio::test]
async fn test_check_status_no_job_complete() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = "no_job_status_hash".to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    // Write a dummy script so the bundle can at least load
    fixture.write_job_submit_check_status(&bundle_hash, "/tmp", "123", r#"{"complete": true}"#);

    // Create a model that is NOT in the database
    let job_model = db::job::Model {
        id: 9999, // Non-existent ID
        job_id: Some(1234),
        scheduler_id: Some(4321),
        bundle_hash,
        ..Default::default()
    };

    // Act - this should handle it gracefully; the function will send a completion
    // message even for a non-existent job, so set up a permissive mock.
    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_queue_message().returning(|_, _, _, _| {});
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));

    crate::jobs::check_job_status(job_model, false).await;
}

#[tokio::test]
async fn test_delete_job_success() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_job_delete(&bundle_hash, "True");

    let job_id = 888i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.bundle_hash = bundle_hash.clone();
    db::save_job(db, job).await.unwrap();

    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    mock_ws.expect_queue_message().returning(move |_, _, _, _| {
        let _ = tx_clone.send(());
    });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));

    use crate::messaging::DELETE_JOB;
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    let msg = Message::from_data(msg_raw.get_data().clone());

    crate::jobs::handle_job_delete(msg).await;

    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Timeout waiting for delete msg");

    let job_after = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    assert!(job_after.deleted);
}

#[tokio::test]
async fn test_archive_fail() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;

    let job = db::job::Model {
        id: 1,
        job_id: Some(1),
        scheduler_id: Some(1),
        bundle_hash: "h".to_string(),
        working_directory: "/not/a/real/path/that/exists/123".to_string(),
        ..Default::default()
    };

    let result = crate::jobs::archive_job(&job).await;
    assert!(!result);
}

// ============================================================================
// Job Check Status Tests - ported from test_check_status.cpp
// ============================================================================

use crate::db::{job, jobstatus};
use crate::jobs::check_job_status;
use crate::messaging::{COMPLETED, RUNNING, UPDATE_JOB};
use tempfile::TempDir;

async fn setup_check_status_test(db_name: &str) -> (BundleFixture, String, job::Model, TempDir) {
    setup_test(db_name).await;

    // Reset websocket client to prevent mock bleeding between tests
    crate::websocket::reset_websocket_client_for_test();

    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    // Create a temporary directory for the job's working directory
    let working_dir = TempDir::new().unwrap();

    // Create a running job
    let db = get_db();
    let job = job::Model {
        id: 0, // Let DB assign ID
        job_id: Some(1234),
        scheduler_id: Some(4321),
        bundle_hash: bundle_hash.clone(),
        working_directory: working_dir.path().to_string_lossy().to_string(),
        running: true,
        ..Default::default()
    };
    let job = db::save_job(db, job).await.unwrap();

    (fixture, bundle_hash, job, working_dir)
}

#[tokio::test]
async fn test_check_status_no_status_not_complete() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script that returns empty status, not complete
    fixture.write_job_status(&bundle_hash, r#"{"status": [], "complete": false}"#);

    let mut mock_ws = MockWebsocketClient::new();
    // No messages expected - no status changes and not complete
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    check_job_status(job.clone(), true).await;

    // No status records should be created
    let db = get_db();
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 0);

    // Job should still be running
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(job_refreshed.running);
}

#[tokio::test]
async fn test_check_status_no_status_complete() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, working_dir) = setup_check_status_test(&db_name).await;

    // Write status script that returns empty status, complete
    fixture.write_job_status(&bundle_hash, r#"{"status": [], "complete": true}"#);

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

    check_job_status(job.clone(), true).await;

    // Wait for message
    let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for UPDATE_JOB message")
        .expect("No message received");

    let mut msg = Message::from_data(msg_data);
    assert_eq!(msg.id, UPDATE_JOB);
    assert_eq!(msg.pop_uint(), 1234); // job_id
    assert_eq!(msg.pop_string(), "_job_completion_");
    assert_eq!(msg.pop_uint(), COMPLETED);
    assert_eq!(msg.pop_string(), "Job has completed");

    // Job should no longer be running
    let db = get_db();
    let job_refreshed = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(!job_refreshed.running);

    // Archive should have been created
    let archive_path = working_dir.path().join("archive.tar.gz");
    assert!(archive_path.exists());
}

#[tokio::test]
async fn test_check_status_job_running_force_notification_duplicates() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with RUNNING status
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

    // Create duplicate status records
    let db = get_db();
    let status1 = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "test_what".to_string(),
        state: RUNNING as i32 - 10, // QUEUED
    };
    let status1 = db::save_status(db, status1).await.unwrap();

    let mut status2 = status1.clone();
    status2.id = 0;
    let _ = db::save_status(db, status2).await.unwrap();

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

    check_job_status(job.clone(), true).await;

    // Duplicates should be deleted, only one status should exist
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 1);
    assert_eq!(v_status[0].what, "test_what");
    assert_eq!(v_status[0].state, RUNNING as i32);

    // Wait for message
    let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for UPDATE_JOB message")
        .expect("No message received");

    let mut msg = Message::from_data(msg_data);
    assert_eq!(msg.id, UPDATE_JOB);
    assert_eq!(msg.pop_uint(), 1234);
    assert_eq!(msg.pop_string(), "test_what");
    assert_eq!(msg.pop_uint(), RUNNING);
    assert_eq!(msg.pop_string(), "Some info");
}

#[tokio::test]
async fn test_check_status_job_running_force_notification() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with RUNNING status
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

    // Create a QUEUED status record
    let db = get_db();
    let status = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "test_what".to_string(),
        state: (RUNNING - 10) as i32, // QUEUED
    };
    let status = db::save_status(db, status).await.unwrap();
    let original_id = status.id;

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

    check_job_status(job.clone(), true).await;

    // Status should be updated (same ID)
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 1);
    assert_eq!(v_status[0].id, original_id);
    assert_eq!(v_status[0].what, "test_what");
    assert_eq!(v_status[0].state, RUNNING as i32);

    // Wait for message
    let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for UPDATE_JOB message")
        .expect("No message received");

    let mut msg = Message::from_data(msg_data);
    assert_eq!(msg.id, UPDATE_JOB);
    assert_eq!(msg.pop_uint(), 1234);
    assert_eq!(msg.pop_string(), "test_what");
    assert_eq!(msg.pop_uint(), RUNNING);
    assert_eq!(msg.pop_string(), "Some info");
}

#[tokio::test]
async fn test_check_status_job_running_force_notification_same_status() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with RUNNING status
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

    // Create a RUNNING status record (same status)
    let db = get_db();
    let status = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "test_what".to_string(),
        state: RUNNING as i32,
    };
    let status = db::save_status(db, status).await.unwrap();
    let original_id = status.id;

    let mut mock_ws = MockWebsocketClient::new();
    // Should still send notification because force_notification=true
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    mock_ws
        .expect_queue_message()
        .times(1)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    check_job_status(job.clone(), true).await;

    // Status should remain the same
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 1);
    assert_eq!(v_status[0].id, original_id);
    assert_eq!(v_status[0].state, RUNNING as i32);

    // Should still send message because force_notification=true
    let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for UPDATE_JOB message")
        .expect("No message received");

    let mut msg = Message::from_data(msg_data);
    assert_eq!(msg.id, UPDATE_JOB);
    assert_eq!(msg.pop_uint(), 1234);
    assert_eq!(msg.pop_string(), "test_what");
    assert_eq!(msg.pop_uint(), RUNNING);
}

#[tokio::test]
async fn test_check_status_new_status_no_existing() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with RUNNING status
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

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

    // force_notification=false, but status is new so should still notify
    check_job_status(job.clone(), false).await;

    // One status record should be created
    let db = get_db();
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 1);
    assert_eq!(v_status[0].what, "test_what");
    assert_eq!(v_status[0].state, RUNNING as i32);

    // Wait for message
    let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for UPDATE_JOB message")
        .expect("No message received");

    let mut msg = Message::from_data(msg_data);
    assert_eq!(msg.id, UPDATE_JOB);
    assert_eq!(msg.pop_uint(), 1234);
    assert_eq!(msg.pop_string(), "test_what");
    assert_eq!(msg.pop_uint(), RUNNING);
    assert_eq!(msg.pop_string(), "Some info");
}

#[tokio::test]
async fn test_check_status_job_running_changed_status() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with RUNNING status
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

    // Create a QUEUED status record
    let db = get_db();
    let status = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "test_what".to_string(),
        state: (RUNNING - 10) as i32, // QUEUED
    };
    let status = db::save_status(db, status).await.unwrap();
    let original_id = status.id;

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

    // force_notification=false, but status changed so should notify
    check_job_status(job.clone(), false).await;

    // Status should be updated
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 1);
    assert_eq!(v_status[0].id, original_id);
    assert_eq!(v_status[0].state, RUNNING as i32);

    // Wait for message
    let msg_data = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for UPDATE_JOB message")
        .expect("No message received");

    let mut msg = Message::from_data(msg_data);
    assert_eq!(msg.id, UPDATE_JOB);
    assert_eq!(msg.pop_uint(), 1234);
    assert_eq!(msg.pop_string(), "test_what");
    assert_eq!(msg.pop_uint(), RUNNING);
    assert_eq!(msg.pop_string(), "Some info");
}

#[tokio::test]
async fn test_check_status_job_running_error_1() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    // Create a test file that will be archived
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    let job_id = 1234i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.scheduler_id = Some(4321);
    job.running = true;
    job.bundle_hash = bundle_hash.clone();
    job.working_directory = working_dir.clone();
    let job = db::save_job(db, job).await.unwrap(); // Save and get back the saved job with id

    // Write status script that returns COMPLETED and ERROR statuses
    // ERROR status (400) should terminate the job
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Step completed", "what": "step1", "status": 500}, {"info": "Error occurred", "what": "step2", "status": 400}], "complete": false}"#);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    // Expect 3 messages: 2 status updates + 1 completion notification
    mock_ws
        .expect_queue_message()
        .times(3)
        .returning(move |_, _data, _, _| {
            let _ = tx_clone.send(());
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    check_job_status(job.clone(), false).await;

    // Verify job is no longer running
    let job_after = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(
        !job_after.running,
        "Job should not be running after ERROR status"
    );

    // Verify archive was created
    let archive_path = std::path::Path::new(&working_dir).join("archive.tar.gz");
    assert!(
        archive_path.exists(),
        "Archive should be created after job termination"
    );

    // Verify 3 messages were sent (2 status + 1 completion)
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for message")
            .expect("No message received");
    }

    // Verify the completion message has ERROR status
    let db_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert!(
        db_status.iter().any(|s| s.state as u32 == ERROR),
        "ERROR status should be recorded"
    );
}

#[tokio::test]
async fn test_check_status_exception() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    // Write a status script that raises an exception
    fixture.write_raw_script(
        &bundle_hash,
        r#"
def status(details, job_data):
    raise Exception("Test exception in status check")
"#,
    );

    let job_id = 1251i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.scheduler_id = Some(4321);
    job.running = true;
    job.bundle_hash = bundle_hash.clone();
    job.working_directory = "/tmp/test_working_dir".to_string();
    let original_scheduler_id = job.scheduler_id;
    let original_running = job.running;
    let job = db::save_job(db, job).await.unwrap();

    let job_id_saved = job.id;

    let mut mock_ws = MockWebsocketClient::new();
    // No messages should be sent when exception occurs
    mock_ws.expect_queue_message().times(0);
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // Should not panic
    check_job_status(job.clone(), false).await;

    // Job should be unchanged
    let job_after = db::get_job_by_id(db, job_id_saved).await.unwrap().unwrap();
    assert_eq!(
        job_after.running, original_running,
        "Job running state should be unchanged"
    );
    assert_eq!(
        job_after.scheduler_id, original_scheduler_id,
        "Job scheduler_id should be unchanged"
    );

    // No status records should be created
    let db_status = db::get_job_status_by_job_id(db, job_id_saved)
        .await
        .unwrap();
    assert_eq!(db_status.len(), 0, "No status records should be created");
}

#[tokio::test]
async fn test_check_status_job_running_error_2() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    // Create a test file that will be archived
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    let job_id = 1235i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.scheduler_id = Some(4321);
    job.running = true;
    job.bundle_hash = bundle_hash.clone();
    job.working_directory = working_dir.clone();
    let job = db::save_job(db, job).await.unwrap();

    // WALL_TIME_EXCEEDED status (value > RUNNING and != COMPLETED) terminates the job
    // Using status value 600 which is > RUNNING (50) and != COMPLETED (500)
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Wall time exceeded", "what": "timeout", "status": 600}, {"info": "Step completed", "what": "step1", "status": 500}], "complete": false}"#);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    // Expect 3 messages: 2 status updates + 1 completion notification
    mock_ws
        .expect_queue_message()
        .times(3)
        .returning(move |_, _data, _, _| {
            let _ = tx_clone.send(());
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    check_job_status(job.clone(), false).await;

    // Verify job is no longer running
    let job_after = db::get_job_by_id(db, job.id).await.unwrap().unwrap();
    assert!(
        !job_after.running,
        "Job should not be running after WALL_TIME_EXCEEDED status"
    );

    // Verify archive was created
    let archive_path = std::path::Path::new(&working_dir).join("archive.tar.gz");
    assert!(
        archive_path.exists(),
        "Archive should be created after job termination"
    );

    // Verify 3 messages were sent (2 status + 1 completion)
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for message")
            .expect("No message received");
    }

    // Verify the termination status was recorded (600)
    let db_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert!(
        db_status.iter().any(|s| s.state as u32 == 600),
        "WALL_TIME_EXCEEDED status (600) should be recorded"
    );
}

#[tokio::test]
async fn test_check_status_job_running_new_status_multiple() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with multiple new statuses
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Stage 1 info", "what": "stage1", "status": 50}, {"info": "Stage 2 info", "what": "stage2", "status": 40}], "complete": false}"#);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    // Expect 2 messages for 2 new statuses
    mock_ws
        .expect_queue_message()
        .times(2)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // force_notification=false, but statuses are new so should notify
    check_job_status(job.clone(), false).await;

    // Two status records should be created
    let db = get_db();
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 2);

    // Wait for both messages
    let msg_data1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for first UPDATE_JOB message")
        .expect("No message received");

    let msg_data2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for second UPDATE_JOB message")
        .expect("No message received");

    let mut msg1 = Message::from_data(msg_data1);
    assert_eq!(msg1.id, UPDATE_JOB);
    assert_eq!(msg1.pop_uint(), 1234); // job_id
    let what1 = msg1.pop_string();
    let state1 = msg1.pop_uint();
    assert!(what1 == "stage1" || what1 == "stage2");
    assert!(state1 == 50 || state1 == 40);

    let mut msg2 = Message::from_data(msg_data2);
    assert_eq!(msg2.id, UPDATE_JOB);
    assert_eq!(msg2.pop_uint(), 1234); // job_id
    let what2 = msg2.pop_string();
    let state2 = msg2.pop_uint();
    assert!(what2 == "stage1" || what2 == "stage2");
    assert!(state2 == 50 || state2 == 40);

    // Verify we got both statuses
    assert_ne!(
        what1, what2,
        "Should have received two different status updates"
    );
}

#[tokio::test]
async fn test_check_status_job_running_changed_status_multiple() {
    let db_name = Uuid::new_v4().to_string();
    let (fixture, bundle_hash, job, _working_dir) = setup_check_status_test(&db_name).await;

    // Write status script with multiple status changes
    fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Running info 1", "what": "what1", "status": 50}, {"info": "Running info 2", "what": "what2", "status": 50}], "complete": false}"#);

    // Create two QUEUED status records (will change to RUNNING)
    let db = get_db();
    let status1 = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "what1".to_string(),
        state: (RUNNING - 10) as i32, // QUEUED
    };
    let status1 = db::save_status(db, status1).await.unwrap();
    let original_id1 = status1.id;

    let status2 = jobstatus::Model {
        id: 0,
        job_id: job.id,
        what: "what2".to_string(),
        state: (RUNNING - 10) as i32, // QUEUED
    };
    let status2 = db::save_status(db, status2).await.unwrap();
    let original_id2 = status2.id;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();

    let mut mock_ws = MockWebsocketClient::new();
    // Expect 2 messages for 2 status changes
    mock_ws
        .expect_queue_message()
        .times(2)
        .returning(move |_, data, _, _| {
            let _ = tx_clone.send(data);
        });
    mock_ws.expect_is_server_ready().returning(|| true);

    set_websocket_client(Arc::new(mock_ws));

    // force_notification=false, but statuses changed so should notify
    check_job_status(job.clone(), false).await;

    // Status records should be updated (same IDs)
    let v_status = db::get_job_status_by_job_id(db, job.id).await.unwrap();
    assert_eq!(v_status.len(), 2);
    let ids: Vec<i64> = v_status.iter().map(|s| s.id).collect();
    assert!(ids.contains(&original_id1));
    assert!(ids.contains(&original_id2));
    // Both should now be RUNNING
    assert!(v_status.iter().all(|s| s.state == RUNNING as i32));

    // Wait for both messages
    let msg_data1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for first UPDATE_JOB message")
        .expect("No message received");

    let msg_data2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timeout waiting for second UPDATE_JOB message")
        .expect("No message received");

    let mut msg1 = Message::from_data(msg_data1);
    assert_eq!(msg1.id, UPDATE_JOB);
    assert_eq!(msg1.pop_uint(), 1234); // job_id
    let what1 = msg1.pop_string();
    let state1 = msg1.pop_uint();
    assert!(what1 == "what1" || what1 == "what2");
    assert_eq!(state1, RUNNING);

    let mut msg2 = Message::from_data(msg_data2);
    assert_eq!(msg2.id, UPDATE_JOB);
    assert_eq!(msg2.pop_uint(), 1234); // job_id
    let what2 = msg2.pop_string();
    let state2 = msg2.pop_uint();
    assert!(what2 == "what1" || what2 == "what2");
    assert_eq!(state2, RUNNING);

    // Verify we got both statuses
    assert_ne!(
        what1, what2,
        "Should have received two different status updates"
    );
}
