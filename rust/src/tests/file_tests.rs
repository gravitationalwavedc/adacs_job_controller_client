use crate::messaging::{Message, Priority, FILE_LIST, FILE_LIST_ERROR, FILE_DOWNLOAD, FILE_DOWNLOAD_DETAILS, FILE_CHUNK, PAUSE_FILE_CHUNK_STREAM, RESUME_FILE_CHUNK_STREAM, UPLOAD_FILE, FILE_UPLOAD_CHUNK, FILE_UPLOAD_ERROR, FILE_UPLOAD_COMPLETE, SYSTEM_SOURCE, SERVER_READY};
use crate::files::{handle_file_list, handle_file_download, handle_file_upload};
use crate::db::{self, get_db, get_or_create_by_job_id};
use crate::bundle_manager::BundleManager;
use crate::websocket::{MockWebsocketClient, set_websocket_client};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::tests::fixtures::websocket_server_fixture::WebsocketServerFixture;
use crate::config::TEST_CONFIG;
use serde_json::json;
use uuid::Uuid;
use std::sync::Arc;
use std::time::Duration;
use mockall::predicate::*;
use tempfile::TempDir;
use std::fs;
use std::path::{Path};

use crate::bundle_db::PyInit_bundledb;
use crate::bundle_logging::PyInit_bundlelogging;

static INIT: std::sync::Once = std::sync::Once::new();

async fn setup_test() {
    INIT.call_once(|| {
        crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
        unsafe {
            crate::python_interface::PyImport_AppendInittab(b"_bundledb\0".as_ptr() as *const std::os::raw::c_char, Some(PyInit_bundledb));
            crate::python_interface::PyImport_AppendInittab(b"_bundlelogging\0".as_ptr() as *const std::os::raw::c_char, Some(PyInit_bundlelogging));
        }
        crate::python_interface::init_python();
    });
    
    // Use a fixed name for shared memory DB
    let _ = db::initialize("sqlite:filetests?mode=memory&cache=shared").await;
    
    let db = get_db();
    use sea_orm::{ConnectionTrait, DbBackend};
    
    let schema_job = r#"
        CREATE TABLE IF NOT EXISTS jobclient_job (
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
        CREATE TABLE IF NOT EXISTS jobclient_jobstatus (
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

fn set_test_config(port: u16) {
    let mut config = TEST_CONFIG.lock().unwrap();
    *config = Some(json!({
        "websocketEndpoint": format!("ws://127.0.0.1:{}/ws/", port)
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_not_exist() {
    setup_test().await;
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-1".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(2234); // Job ID
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(".");
    msg_raw.push_bool(false);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST_ERROR);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_string(), "Job does not exist");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_submitting() {
    setup_test().await;
    
    let job_id = 1234i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = true;
    db::save_job(db, job).await.unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-2".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(".");
    msg_raw.push_bool(false);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST_ERROR);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_string(), "Job is not submitted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_outside_working_directory() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1235i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-3".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string("../");
    msg_raw.push_bool(false);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST_ERROR);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_string(), "Path to list files is outside the working directory");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_directory_not_exist() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1236i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-4".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string("not_exist");
    msg_raw.push_bool(false);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST_ERROR);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_string(), "Path to list files does not exist");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_directory_is_a_file() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    let file_path = temp_dir.path().join("test_file");
    fs::write(&file_path, "test").unwrap();
    
    let job_id = 1237i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-5".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string("test_file");
    msg_raw.push_bool(false);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST_ERROR);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_string(), "Path to list files is not a directory");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_success_recursive() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let sub_dir = temp_dir.path().join("sub");
    fs::create_dir(&sub_dir).unwrap();
    fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
    fs::write(sub_dir.join("file2.txt"), "content2").unwrap();
    
    let job_id = 1238i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-6".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(".");
    msg_raw.push_bool(true); // recursive
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_uint(), 3); // file1, sub, sub/file2
    
    let mut items = vec![];
    for _ in 0..3 {
        items.push((response_msg.pop_string(), response_msg.pop_bool(), response_msg.pop_ulong()));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    
    assert_eq!(items[0].0, "file1.txt");
    assert_eq!(items[0].1, false);
    assert_eq!(items[1].0, "sub");
    assert_eq!(items[1].1, true);
    assert_eq!(items[2].0, "sub/file2.txt");
    assert_eq!(items[2].1, false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_job_success_not_recursive() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let sub_dir = temp_dir.path().join("sub");
    fs::create_dir(&sub_dir).unwrap();
    fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
    fs::write(sub_dir.join("file2.txt"), "content2").unwrap();
    
    let job_id = 1239i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-7".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(".");
    msg_raw.push_bool(false); // not recursive
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_uint(), 2); // file1, sub
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_list_no_job_success() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
    
    let fixture = BundleFixture::new();
    let bundle_hash = "no_job_hash";
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
    
    let script = format!(r#"
def working_directory(details, job_data):
    return "{}"
"#, working_dir);
    fs::create_dir_all(fixture.get_bundle_path().join(bundle_hash)).unwrap();
    fs::write(fixture.get_bundle_path().join(bundle_hash).join("bundle.py"), script).unwrap();
    
    let mut mock_ws = MockWebsocketClient::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_clone = tx.clone();
    
    let test_uuid = "test-uuid-8".to_string();
    let uuid_clone = test_uuid.clone();
    mock_ws.expect_queue_message()
        .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
        .times(1)
        .returning(move |_, data, _, _| {
            let msg = Message::from_data(data);
            let _ = tx_clone.send(msg);
        });
    
    set_websocket_client(Arc::new(mock_ws));
    
    let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(0); // No Job
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string(bundle_hash);
    msg_raw.push_string(".");
    msg_raw.push_bool(false);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_list(msg).await;
    
    let response = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.expect("Timeout").expect("No response");
    assert_eq!(response.id, FILE_LIST);
    let mut response_msg = response;
    assert_eq!(response_msg.pop_string(), test_uuid);
    assert_eq!(response_msg.pop_uint(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_download_job_success() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    let file_content = b"random test data";
    fs::write(temp_dir.path().join("test_download.txt"), file_content).unwrap();
    
    let job_id = 1240i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-download".to_string();
    let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string("test_download.txt");
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_download(msg).await;
    
    let mut server = server;
    // Server should receive FILE_DOWNLOAD_DETAILS
    let details = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for details").expect("No details");
    assert_eq!(details.id, FILE_DOWNLOAD_DETAILS);
    let mut details_msg = details;
    assert_eq!(details_msg.pop_ulong(), file_content.len() as u64);
    
    // Server should receive FILE_CHUNK
    let chunk = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for chunk").expect("No chunk");
    assert_eq!(chunk.id, FILE_CHUNK);
    let mut chunk_msg = chunk;
    assert_eq!(chunk_msg.pop_bytes(), file_content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_file_download_no_job_success() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    let file_content = b"bundle file content";
    fs::write(temp_dir.path().join("bundle_download.txt"), file_content).unwrap();
    
    let fixture = BundleFixture::new();
    let bundle_hash = "no_job_hash_download";
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
    
    let script = format!(r#"
def working_directory(details, job_data):
    return "{}"
"#, working_dir);
    fs::create_dir_all(fixture.get_bundle_path().join(bundle_hash)).unwrap();
    fs::write(fixture.get_bundle_path().join(bundle_hash).join("bundle.py"), script).unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-bundle-download".to_string();
    let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(0); // No job
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string(bundle_hash);
    msg_raw.push_string("bundle_download.txt");
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_download(msg).await;
    
    let mut server = server;
    let details = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for details").expect("No details");
    assert_eq!(details.id, FILE_DOWNLOAD_DETAILS);
    let mut details_msg = details;
    assert_eq!(details_msg.pop_ulong(), file_content.len() as u64);
    
    let chunk = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for chunk").expect("No chunk");
    assert_eq!(chunk.id, FILE_CHUNK);
    let mut chunk_msg = chunk;
    assert_eq!(chunk_msg.pop_bytes(), file_content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_file_upload_job_based_success() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1241i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir.clone();
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-upload".to_string();
    let file_content = b"uploaded content";
    let target_path = "subdir/uploaded.txt";
    
    let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(target_path);
    msg_raw.push_ulong(file_content.len() as u64);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_upload(msg).await;
    
    let mut server = server;
    let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for ready").expect("No ready");
    assert_eq!(ready.id, SERVER_READY);

    let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
    chunk_msg.push_bytes(file_content);
    server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();
    
    let mut complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
    server.msg_tx.send(complete_msg.get_data().clone()).unwrap();
    
    let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for response").expect("No response");
    assert_eq!(response.id, FILE_UPLOAD_COMPLETE);
    
    let final_path = Path::new(&working_dir).join(target_path);
    assert!(final_path.exists());
    assert_eq!(fs::read(final_path).unwrap(), file_content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_file_upload_large_file() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1242i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir.clone();
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-upload-large".to_string();
    let file_content = vec![0u8; 1024 * 1024]; // 1MB
    let target_path = "large.bin";
    
    let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(target_path);
    msg_raw.push_ulong(file_content.len() as u64);
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_upload(msg).await;
    
    let mut server = server;
    let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

    for chunk in file_content.chunks(64 * 1024) {
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(chunk);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();
    }
    
    let mut complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
    server.msg_tx.send(complete_msg.get_data().clone()).unwrap();
    
    let response = tokio::time::timeout(Duration::from_secs(5), server.msg_rx.recv()).await.expect("Timeout waiting for response").expect("No response");
    assert_eq!(response.id, FILE_UPLOAD_COMPLETE);
    
    let final_path = Path::new(&working_dir).join(target_path);
    assert_eq!(fs::metadata(final_path).unwrap().len(), 1024 * 1024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_file_upload_file_size_mismatch() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1243i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir.clone();
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-upload-mismatch".to_string();
    
    let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string("some_hash");
    msg_raw.push_string("mismatch.txt");
    msg_raw.push_ulong(1000); // Expect 1000 bytes
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_upload(msg).await;
    
    let mut server = server;
    let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

    let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
    chunk_msg.push_bytes(&[0u8; 500]);
    server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();
    
    let mut complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
    server.msg_tx.send(complete_msg.get_data().clone()).unwrap();
    
    let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for response").expect("No response");
    assert_eq!(response.id, FILE_UPLOAD_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_file_upload_zero_byte_file() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1244i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir.clone();
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-upload-zero".to_string();
    let target_path = "zero.txt";
    
    let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(target_path);
    msg_raw.push_ulong(0); // 0 bytes
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_upload(msg).await;
    
    let mut server = server;
    let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

    let mut complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
    server.msg_tx.send(complete_msg.get_data().clone()).unwrap();
    
    let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for response").expect("No response");
    assert_eq!(response.id, FILE_UPLOAD_COMPLETE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_file_upload_actual_bigger_than_declared() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    
    let job_id = 1245i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir.clone();
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-upload-bigger".to_string();
    let target_path = "bigger.txt";
    
    let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string("some_hash");
    msg_raw.push_string(target_path);
    msg_raw.push_ulong(10); // Declare 10 bytes
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_upload(msg).await;
    
    let mut server = server;
    let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

    let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
    chunk_msg.push_bytes(&[0u8; 20]);
    server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();
    
    let mut complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
    server.msg_tx.send(complete_msg.get_data().clone()).unwrap();
    
    let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await.expect("Timeout waiting for response").expect("No response");
    assert_eq!(response.id, FILE_UPLOAD_ERROR);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_file_download_pause_resume() {
    setup_test().await;
    
    let temp_dir = TempDir::new().unwrap();
    let working_dir = temp_dir.path().to_str().unwrap().to_string();
    let file_content = vec![0u8; 320 * 1024]; // 320KB (5 chunks of 64KB)
    fs::write(temp_dir.path().join("pause_test.bin"), &file_content).unwrap();
    
    let job_id = 1246i64;
    let db = get_db();
    let mut job = get_or_create_by_job_id(db, job_id).await.unwrap();
    job.job_id = Some(job_id);
    job.submitting = false;
    job.working_directory = working_dir;
    db::save_job(db, job).await.unwrap();
    
    let server = WebsocketServerFixture::new().await;
    set_test_config(server.port);
    
    let test_uuid = "test-uuid-pause".to_string();
    let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
    msg_raw.push_uint(job_id as u32);
    msg_raw.push_string(&test_uuid);
    msg_raw.push_string("some_hash");
    msg_raw.push_string("pause_test.bin");
    
    let msg = Message::from_data(msg_raw.get_data().clone());
    
    handle_file_download(msg).await;
    
    let mut server = server;
    let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await; // DETAILS
    let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await; // CHUNK 1
    
    let mut pause_msg = Message::new(PAUSE_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
    server.msg_tx.send(pause_msg.get_data().clone()).unwrap();
    
    // Give time for the pause to propagate through the websocket
    tokio::time::sleep(Duration::from_millis(200)).await;
    
    // Drain any chunks that were in-flight before the pause took effect
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), server.msg_rx.recv()).await {}
    
    // Now that pause is definitely active, no more chunks should arrive
    let res = tokio::time::timeout(Duration::from_millis(500), server.msg_rx.recv()).await;
    assert!(res.is_err(), "Expected no chunks while paused");
    
    let mut resume_msg = Message::new(RESUME_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
    server.msg_tx.send(resume_msg.get_data().clone()).unwrap();
    
    let chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv()).await.expect("No chunk after resume").expect("No chunk");
    assert_eq!(chunk.id, FILE_CHUNK);
}
