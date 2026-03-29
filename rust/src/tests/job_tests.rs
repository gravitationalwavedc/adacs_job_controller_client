use crate::messaging::{Message, Priority, SUBMIT_JOB, SYSTEM_SOURCE};
use crate::jobs::{handle_job_submit, MAX_SUBMIT_COUNT};
use crate::db::{self, get_db, get_or_create_by_job_id};
use crate::bundle_manager::BundleManager;
use crate::websocket::{MockWebsocketClient, set_websocket_client};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use std::fs;
use uuid::Uuid;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use mockall::predicate::*;

use crate::bundle_db::PyInit_bundledb;
use crate::bundle_logging::PyInit_bundlelogging;

static INIT: std::sync::Once = std::sync::Once::new();

async fn setup_test(db_name: &str) {
    INIT.call_once(|| {
        crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
        unsafe {
            crate::python_interface::PyImport_AppendInittab(b"_bundledb\0".as_ptr() as *const std::os::raw::c_char, Some(PyInit_bundledb));
            crate::python_interface::PyImport_AppendInittab(b"_bundlelogging\0".as_ptr() as *const std::os::raw::c_char, Some(PyInit_bundlelogging));
        }
        crate::python_interface::init_python();
    });
    
    // Use a unique in-memory database for each test to avoid "table already exists"
    db::initialize(&format!("sqlite:{}?mode=memory&cache=shared", db_name)).await.expect("Failed to initialize DB");
    
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
    
    let _ = db.execute(sea_orm::Statement::from_string(DbBackend::Sqlite, schema_job.to_string())).await;
    let _ = db.execute(sea_orm::Statement::from_string(DbBackend::Sqlite, schema_status.to_string())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    mock_ws.expect_queue_message()
        .with(
            always(),
            always(),
            eq(Priority::Medium),
            always()
        )
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
        assert_eq!(job.submitting_count, count, "Client never incremented submittingCount at count {}", count);
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
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("Timed out waiting for queue_message");

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
    assert_eq!(job.submitting, false);
    assert_eq!(job.scheduler_id, Some(4321));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    mock_ws.expect_queue_message()
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
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("Timed out waiting for first queue_message");
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("Timed out waiting for second queue_message");

    let db = get_db();
    let job_result = crate::db::get_job_by_id(db, job_id as i64).await.unwrap();
    // In C++ test, it deletes the job on submit error
    assert!(job_result.is_none() || job_result.unwrap().id == 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    assert!(std::path::Path::new(&working_dir).join("archive.tar.gz").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    mock_ws.expect_queue_message().returning(move |_, _, _, _| { let _ = tx_clone.send(()); });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));
    
    use crate::messaging::CANCEL_JOB;
    let mut msg_raw = Message::new(CANCEL_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    crate::jobs::handle_job_cancel(msg).await;
    
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("Timeout waiting for cancel msg");
    
    let job_after = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    assert!(!job_after.running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    mock_ws.expect_queue_message()
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
        
        fixture.write_job_submit_check_status(&bundle_hash, &working_dir, &format!("{}", job_id), r#"{"complete": true}"#);
        
        let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
        job.job_id = Some(job_id);
        job.running = true;
        job.bundle_hash = bundle_hash;
        job.working_directory = working_dir;
        db::save_job(db, job).await.unwrap();
    }

    crate::jobs::check_all_jobs_status().await;

    for _ in 0..100 {
        tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("Timeout waiting for stress test status update");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_check_status_job_running_new_status() {
    let db_name = Uuid::new_v4().to_string();
    setup_test(&db_name).await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
    
    let temp_dir = tempfile::TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    // Status check returns a new status (key is "status" matching C++ CheckStatus.cpp)
    fixture.write_job_submit_check_status(&bundle_hash, &working_dir, "123", r#"{"complete": false, "status": [{"what": "stage1", "status": 50}]}"#);
    
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
    mock_ws.expect_queue_message().returning(move |_, _, _, _| { let _ = tx_clone.send(()); });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));
    
    let job_model = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    crate::jobs::check_job_status(job_model.clone(), false).await;
    
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("Timeout waiting for status update");
    
    // Query by internal PK (job_model.id), not external job_id - status FK references job.id
    let status_after = db::get_job_status_by_job_id(db, job_model.id).await.unwrap();
    assert_eq!(status_after.len(), 1);
    assert_eq!(status_after[0].what, "stage1");
    assert_eq!(status_after[0].state, 50); // RUNNING = 50
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    
    // Act - this should handle it gracefully
    crate::jobs::check_job_status(job_model, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    mock_ws.expect_queue_message().returning(move |_, _, _, _| { let _ = tx_clone.send(()); });
    mock_ws.expect_is_server_ready().returning(|| true);
    set_websocket_client(Arc::new(mock_ws));
    
    use crate::messaging::DELETE_JOB;
    let mut msg_raw = Message::new(DELETE_JOB, Priority::Medium, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    crate::jobs::handle_job_delete(msg).await;
    
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("Timeout waiting for delete msg");
    
    let job_after = db::get_job_by_job_id(db, job_id).await.unwrap().unwrap();
    assert!(job_after.deleted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
