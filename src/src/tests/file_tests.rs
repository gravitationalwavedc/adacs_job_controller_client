use crate::bundle_manager::BundleManager;
use crate::config::TEST_CONFIG;
use crate::db::job;
use crate::files::{handle_file_download, handle_file_list, handle_file_upload};
use crate::messaging::{
    Message, Priority, DB_JOBSTATUS_SAVE, DB_JOB_GET_BY_ID, DB_JOB_GET_BY_JOB_ID, DB_JOB_SAVE,
    DB_RESPONSE, FILE_CHUNK, FILE_DOWNLOAD, FILE_DOWNLOAD_DETAILS, FILE_DOWNLOAD_ERROR, FILE_LIST,
    FILE_LIST_ERROR, FILE_UPLOAD_CHUNK, FILE_UPLOAD_COMPLETE, FILE_UPLOAD_ERROR,
    PAUSE_FILE_CHUNK_STREAM, RESUME_FILE_CHUNK_STREAM, SERVER_READY, SYSTEM_SOURCE, UPLOAD_FILE,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::tests::fixtures::temporary_directory_fixture::TemporaryDirectoryFixture;
use crate::tests::fixtures::websocket_server_fixture::WebsocketServerFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use mockall::predicate::*;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
/// Call this AFTER creating your mock, before `set_websocket_client`
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
                    let _id = m.pop_ulong() as i64;
                    let _job_id = m.pop_ulong() as i64;
                    let _state = m.pop_int();
                    let _what = m.pop_string();
                    resp.push_bool(true);
                    resp.push_ulong(1);
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

fn setup_test(_db_name: &str) {
    crate::tests::init_python_global();
}

/// Create a new mock DB state for tests
fn create_mock_state() -> Arc<std::sync::Mutex<MockDbState>> {
    Arc::new(std::sync::Mutex::new(MockDbState::new()))
}

fn set_test_config(port: u16) {
    let mut config = TEST_CONFIG.lock().unwrap();
    *config = Some(json!({
        "websocketEndpoint": format!("ws://127.0.0.1:{}/ws/", port)
    }));
}

#[test_fork::test]
fn test_get_file_list_job_not_exist() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test1");

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-1".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
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

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(response_msg.pop_string(), "Job does not exist");
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_job_submitting() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test2");

        // Set up mock DB FIRST before any DB calls
        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-2".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
            .times(1)
            .returning(move |_, data, _, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        // Insert job directly into mock DB state
        let job_id = 1234i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: true,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: String::new(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(".");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(response_msg.pop_string(), "Job is not submitted");
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_job_outside_working_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test3");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-3".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
            .times(1)
            .returning(move |_, data, _, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1235i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("../");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Path to list files is outside the working directory"
        );
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_job_directory_not_exist() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test4");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-4".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
            .times(1)
            .returning(move |_, data, _, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1236i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("not_exist");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Path to list files does not exist"
        );
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_job_directory_is_a_file() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test5");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-5".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
            .times(1)
            .returning(move |_, data, _, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1237i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("file1.txt");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Path to list files is not a directory"
        );
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_job_success_recursive() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test6");

        let fixture = TemporaryDirectoryFixture::new();
        let list_root = fixture.create_test_directory("list_root");
        let working_dir = list_root.to_str().unwrap().to_string();
        fixture.create_test_directory("list_root/sub");
        fixture.create_test_file("list_root/file1.txt", "content1");
        fixture.create_test_file("list_root/sub/file2.txt", "content2");

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-6".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
            .times(1)
            .returning(move |_, data, _, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1238i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(".");
        msg_raw.push_bool(true); // recursive

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(response_msg.pop_uint(), 3); // file1, sub, sub/file2

        let mut items = vec![];
        for _ in 0..3 {
            items.push((
                response_msg.pop_string(),
                response_msg.pop_bool(),
                response_msg.pop_ulong(),
            ));
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(items[0].0, "file1.txt");
        assert!(!items[0].1);
        assert_eq!(items[1].0, "sub");
        assert!(items[1].1);
        assert_eq!(items[2].0, "sub/file2.txt");
        assert!(!items[2].1);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_job_success_not_recursive() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test7");

        let fixture = TemporaryDirectoryFixture::new();
        let list_root = fixture.create_test_directory("list_root");
        let working_dir = list_root.to_str().unwrap().to_string();
        fixture.create_test_directory("list_root/sub");
        fixture.create_test_file("list_root/file1.txt", "content1");
        fixture.create_test_file("list_root/sub/file2.txt", "content2");

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-7".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest), always())
            .times(1)
            .returning(move |_, data, _, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1239i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(".");
        msg_raw.push_bool(false); // not recursive

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(response_msg.pop_uint(), 2); // file1, sub
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_no_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test8");

        let state = create_mock_state();
        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();

        let fixture = BundleFixture::new();
        let bundle_hash = "no_job_hash";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-8".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
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

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(response_msg.pop_uint(), 2); // file1.txt + subdir (symlinks are excluded)
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_no_job_outside_working_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test17");

        let state = create_mock_state();
        let _fixture = TemporaryDirectoryFixture::new();

        let fixture = BundleFixture::new();
        let bundle_hash = "no_job_hash_outside";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns /usr as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, "/usr");

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-outside".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
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
        // Request path that goes outside the bundle's working directory
        msg_raw.push_string("../etc");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Path to list files is outside the working directory"
        );
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_no_job_directory_not_exist() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test18");

        let state = create_mock_state();
        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();

        let fixture = BundleFixture::new();
        let bundle_hash = "no_job_hash_not_exist";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns temp_dir as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-not-exist".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
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
        // Request a non-existent directory
        msg_raw.push_string("not_real_directory");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Path to list files does not exist"
        );
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_list_no_job_directory_is_a_file() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test19");

        let state = create_mock_state();
        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();

        let fixture = BundleFixture::new();
        let bundle_hash = "no_job_hash_file_is_dir";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns temp_dir as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-file-is-dir".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
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
        // Request a file path (not a directory)
        msg_raw.push_string("file1.txt");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_list(msg).await;

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Path to list files is not a directory"
        );
    } // end inner()
    inner();
}

// ============================================================================
// File Download Error Tests - ported from test_file_download.cpp
// ============================================================================

#[test_fork::test]
fn test_get_file_download_job_not_exist() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test20");

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-dl-not-exist".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(2234); // Non-existent job ID
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("test.txt");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        // from_data already extracted source and id, so just pop the error message
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Job does not exist");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_job_submitting() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test21");

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1234i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: true,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: String::new(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-dl-submitting".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("test.txt");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Job is not submitted");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_job_outside_working_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test22");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        // Create a file outside the working directory that we can traverse to
        let outside_file = fixture
            .get_temp_path()
            .parent()
            .unwrap()
            .join("outside_test_file.txt");
        fs::write(&outside_file, "outside content").unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1235i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-dl-outside".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        // Path traversal attempt - go up one level then to the outside file
        let outside_filename = outside_file.file_name().unwrap().to_str().unwrap();
        msg_raw.push_string(&format!("../{outside_filename}"));

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(
            error_msg,
            "Path to file download is outside the working directory"
        );
        assert_eq!(response_msg.source, test_uuid);

        // Cleanup
        let _ = fs::remove_file(&outside_file);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_job_file_not_exist() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test23");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1236i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-dl-file-not-exist".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        // Non-existent file
        msg_raw.push_string("not_real_file.txt");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Path to file download does not exist");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_job_file_is_a_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test24");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        // fixture already provides subdir/

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1237i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-dl-file-is-dir".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        // Request a directory instead of a file
        msg_raw.push_string("subdir");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Path to file download is not a file");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test9");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = b"random test data";
        fs::write(
            fixture.get_temp_path().join("test_download.txt"),
            file_content,
        )
        .unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1240i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-download".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("test_download.txt");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        // Server should receive FILE_DOWNLOAD_DETAILS
        let details = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for details")
            .expect("No details");
        assert_eq!(details.id, FILE_DOWNLOAD_DETAILS);
        let mut details_msg = details;
        assert_eq!(details_msg.pop_ulong(), file_content.len() as u64);

        // Server should receive FILE_CHUNK
        let chunk = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for chunk")
            .expect("No chunk");
        assert_eq!(chunk.id, FILE_CHUNK);
        let mut chunk_msg = chunk;
        assert_eq!(chunk_msg.pop_bytes(), file_content);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_no_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test10");

        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();
        let file_content = b"bundle file content";
        fs::write(
            dir.get_temp_path().join("bundle_download.txt"),
            file_content,
        )
        .unwrap();

        let fixture = BundleFixture::new();
        let bundle_hash = "no_job_hash_download";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        // Set up mock WS client for DB calls (handle_file_download calls db::get_job_by_job_id)
        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        mock_ws
            .expect_queue_message()
            .times(..)
            .returning(|_, _, _, _| {});
        mock_ws.expect_is_server_ready().returning(|| true);
        set_websocket_client(Arc::new(mock_ws));

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-bundle-download".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(0); // No job
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string(bundle_hash);
        msg_raw.push_string("bundle_download.txt");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let details = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for details")
            .expect("No details");
        assert_eq!(details.id, FILE_DOWNLOAD_DETAILS);
        let mut details_msg = details;
        assert_eq!(details_msg.pop_ulong(), file_content.len() as u64);

        let chunk = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for chunk")
            .expect("No chunk");
        assert_eq!(chunk.id, FILE_CHUNK);
        let mut chunk_msg = chunk;
        assert_eq!(chunk_msg.pop_bytes(), file_content);
    } // end inner()
    inner();
}

// ============================================================================
// Bundle-mode File Download Error Tests - ported from test_file_download.cpp
// ============================================================================

#[test_fork::test]
fn test_get_file_download_no_job_outside_working_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_download_no_job_outside");

        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();

        // Create a file outside the working directory
        let outside_file = dir
            .get_temp_path()
            .parent()
            .unwrap()
            .join("outside_bundle_download_file.txt");
        fs::write(&outside_file, "outside bundle content").unwrap();

        let fixture = BundleFixture::new();
        let bundle_hash = "bundle_download_outside_hash";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns temp_dir as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-bundle-dl-outside".to_string();
        let outside_filename = outside_file.file_name().unwrap().to_str().unwrap();

        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(0); // No job (bundle mode)
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string(bundle_hash);
        // Path traversal attempt
        msg_raw.push_string(&format!("../{outside_filename}"));

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(
            error_msg,
            "Path to file download is outside the working directory"
        );
        assert_eq!(response_msg.source, test_uuid);

        // Cleanup
        let _ = fs::remove_file(&outside_file);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_no_job_directory_not_exist() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_download_no_job_not_exist");

        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();

        let fixture = BundleFixture::new();
        let bundle_hash = "bundle_download_not_exist_hash";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns temp_dir as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-bundle-dl-not-exist".to_string();

        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(0); // No job (bundle mode)
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string(bundle_hash);
        // Non-existent directory
        msg_raw.push_string("not_real_directory/file.txt");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Path to file download does not exist");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_get_file_download_no_job_file_is_a_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_download_no_job_is_dir");

        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();
        dir.create_test_directory("bundle_subdir");

        let fixture = BundleFixture::new();
        let bundle_hash = "bundle_download_is_dir_hash";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns temp_dir as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-bundle-dl-is-dir".to_string();

        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(0); // No job (bundle mode)
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string(bundle_hash);
        // Request a directory instead of a file
        msg_raw.push_string("bundle_subdir");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Path to file download is not a file");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_job_based_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test11");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1241i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

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

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for ready")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(file_content);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_COMPLETE);

        let final_path = Path::new(&working_dir).join(target_path);
        assert!(final_path.exists());
        assert_eq!(fs::read(final_path).unwrap(), file_content);
    } // end inner()
    inner();
}

// ============================================================================
// File Upload Error Tests - ported from test_file_upload.cpp
// ============================================================================

#[test_fork::test]
fn test_file_upload_bundle_based_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test25");

        let dir = TemporaryDirectoryFixture::new();
        let working_dir = dir.get_temp_path().to_str().unwrap().to_string();

        let fixture = BundleFixture::new();
        let bundle_hash = "bundle_upload_hash";
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle returns temp_dir as working directory
        fixture.write_file_list_no_job_working_directory(bundle_hash, &working_dir);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-bundle-upload".to_string();
        let file_content = b"bundle uploaded content";
        let target_path = "bundle_file.txt";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(0); // No job
        msg_raw.push_string(bundle_hash);
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(file_content.len() as u64);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(file_content);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_COMPLETE);

        let final_path = Path::new(&working_dir).join(target_path);
        assert!(final_path.exists());
        assert_eq!(fs::read(final_path).unwrap(), file_content);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_invalid_path_outside_working_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test26");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        // Create a file outside the working directory to use as traversal target
        let outside_file = fixture
            .get_temp_path()
            .parent()
            .unwrap()
            .join("outside_upload_target.txt");
        fs::write(&outside_file, "existing outside content").unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1241i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-upload-outside".to_string();
        // Use ../filename where filename exists outside working dir
        let outside_filename = outside_file.file_name().unwrap().to_str().unwrap();
        let target_path = format!("../{outside_filename}");

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(&target_path);
        msg_raw.push_ulong(100);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Send chunk data
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(&[0u8; 50]);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(
            error_msg,
            "Target path for file upload is outside the working directory"
        );
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_invalid_job_id() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test27");

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-upload-invalid-job".to_string();
        let target_path = "test.txt";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(99999); // Non-existent job ID
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(100);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Error should be sent immediately after SERVER_READY since job doesn't exist
        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for error response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Job does not exist");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_job_submitting() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test28");

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1242i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: true,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: String::new(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-upload-submitting".to_string();
        let target_path = "test.txt";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(100);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Error should be sent immediately after SERVER_READY since job is submitting
        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for error response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Job is not submitted");
        assert_eq!(response_msg.source, test_uuid);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_symlink_outside_working_directory() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test29");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        // Create a directory outside the working directory
        let outside_dir = fixture
            .get_temp_path()
            .parent()
            .unwrap()
            .join("outside_upload_dir");
        fs::create_dir_all(&outside_dir).unwrap();

        // Create a symlink inside working_dir that points to outside_dir
        let symlink_path = fixture.get_temp_path().join("symlink_to_outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_dir, &symlink_path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside_dir, &symlink_path).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1244i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-upload-symlink".to_string();
        // Target path goes through symlink to escape working directory
        let target_path = "symlink_to_outside/escaped_file.txt";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(100);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Send chunk data
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(&[0u8; 50]);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(
            error_msg,
            "Target path for file upload is outside the working directory"
        );
        assert_eq!(response_msg.source, test_uuid);

        // Verify file was NOT created outside working directory
        let escaped_file = outside_dir.join("escaped_file.txt");
        assert!(!escaped_file.exists());
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_partial_file_cleanup_on_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test30");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1245i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-upload-cleanup".to_string();
        let target_path = "partial_file.txt";
        let declared_size = 1000u64;
        let actual_size = 500u64; // Send less than declared

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(declared_size);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Send partial data (less than declared size)
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        let partial_data = vec![0u8; actual_size as usize];
        chunk_msg.push_bytes(&partial_data);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        // Send complete message (triggers size mismatch error)
        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "File size mismatch: expected 1000, got 500");
        assert_eq!(response_msg.source, test_uuid);

        // Verify partial file was cleaned up (deleted)
        let full_path = fixture.get_temp_path().join(target_path);
        assert!(
            !full_path.exists(),
            "Partial file should have been cleaned up after error"
        );
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_multiple_concurrent_file_uploads() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test31");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1246i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        // Create 5 files with unique content to upload
        let num_uploads = 5;
        let mut expected_data = Vec::new();

        for i in 0..num_uploads {
            let target_path = format!("file_{i}.txt");
            let file_data = vec![i as u8; 1024]; // 1KB of unique data per file
            expected_data.push((target_path, file_data));
        }

        // Run 5 uploads in PARALLEL with unique UUIDs and data.
        // Each upload has its own WebSocket server and the uploads run truly in parallel.
        let mut upload_handles = Vec::new();

        // Pre-create all servers and extract their components
        let mut server_data = Vec::new();
        for _ in 0..num_uploads {
            let server = WebsocketServerFixture::new().await;
            server_data.push((server.port, server.msg_rx, server.msg_tx));
        }

        // Verify job exists before starting parallel uploads
        let job_check = state
            .lock()
            .unwrap()
            .jobs
            .values()
            .find(|j| j.job_id == Some(job_id))
            .cloned();
        assert!(
            job_check.is_some(),
            "Job should exist before parallel uploads"
        );

        for (i, (target_path, file_data)) in expected_data.iter().enumerate().take(num_uploads) {
            let target_path = target_path.clone();
            let file_data = file_data.clone();
            let working_dir = working_dir.clone();
            let (port, mut msg_rx, msg_tx) = server_data.remove(0);

            let handle = tokio::spawn(async move {
                let test_uuid = format!("test-uuid-concurrent-{i}");
                let bundle_hash = format!("bundle_{i}");
                let ws_url = format!("ws://127.0.0.1:{port}/ws/?token={test_uuid}");

                let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
                msg_raw.push_string(&test_uuid);
                msg_raw.push_uint(job_id as u32);
                msg_raw.push_string(&bundle_hash);
                msg_raw.push_string(&target_path);
                msg_raw.push_ulong(file_data.len() as u64);

                let msg = Message::from_data(msg_raw.get_data().clone());

                // Start the upload handler with explicit URL (bypasses global config)
                crate::files::handle_file_upload_with_url(msg, ws_url);

                // Handle WebSocket communication for this upload
                let _ready = tokio::time::timeout(Duration::from_secs(2), msg_rx.recv())
                    .await
                    .expect("Timeout waiting for SERVER_READY")
                    .expect("No ready");

                // Send all data in one chunk
                let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
                chunk_msg.push_bytes(&file_data);
                msg_tx.send(chunk_msg.get_data().clone()).unwrap();

                let complete_msg =
                    Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
                msg_tx.send(complete_msg.get_data().clone()).unwrap();

                let mut response = tokio::time::timeout(Duration::from_secs(2), msg_rx.recv())
                    .await
                    .expect("Timeout waiting for response")
                    .expect("No response");

                if response.id != FILE_UPLOAD_COMPLETE {
                    let err_msg = response.pop_string();
                    panic!("Upload {} failed with {:?}: {}", i, response.id, err_msg);
                }

                (target_path, file_data, working_dir)
            });

            upload_handles.push(handle);
        }

        // Wait for all uploads to complete in parallel
        let results = futures::future::join_all(upload_handles).await;

        // Verify all uploads completed successfully
        for (i, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "Upload {i} should complete: {result:?}");
        }

        // Verify all files were created with correct content
        for (i, result) in results.iter().enumerate() {
            let (target_path, expected_content, working_dir) = result.as_ref().unwrap();
            let full_path = std::path::Path::new(working_dir).join(target_path);
            assert!(full_path.exists(), "File {target_path} should exist");
            let actual_content = fs::read(&full_path).unwrap();
            assert_eq!(
                &actual_content, expected_content,
                "File {i} content should match"
            );
        }
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_nested_directory_creation() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test32");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1247i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-nested-dir".to_string();
        // Target path with nested directories that don't exist yet
        let target_path = "subdir/nested/deep/file.txt";
        let file_content = b"uploaded to nested dirs";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(file_content.len() as u64);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Send all data in one chunk
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(file_content);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");

        assert_eq!(response.id, FILE_UPLOAD_COMPLETE);

        // Verify nested directories were created and file exists
        let full_path = fixture.get_temp_path().join(target_path);
        assert!(full_path.exists(), "File should exist in nested directory");
        assert_eq!(fs::read(&full_path).unwrap(), file_content);

        // Verify intermediate directories exist
        assert!(fixture.get_temp_path().join("subdir").exists());
        assert!(fixture.get_temp_path().join("subdir/nested").exists());
        assert!(fixture.get_temp_path().join("subdir/nested/deep").exists());
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_write_permission_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test33");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        // Create a directory with no write permissions
        let protected_dir = fixture.get_temp_path().join("protected");
        fs::create_dir_all(&protected_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&protected_dir, fs::Permissions::from_mode(0o555)).unwrap();
        }

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1248i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-perm-error".to_string();
        let target_path = "protected/file.txt";
        let file_content = b"should fail to write";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(file_content.len() as u64);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Send chunk data
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(file_content);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");

        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Failed to open target file for writing");

        // Restore permissions so temp dir can be cleaned up
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&protected_dir, fs::Permissions::from_mode(0o755));
        }
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_open_write_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test34");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        // Create a directory with no write permissions
        let protected_dir = fixture.get_temp_path().join("protected");
        fs::create_dir_all(&protected_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&protected_dir, fs::Permissions::from_mode(0o555)).unwrap();
        }

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1249i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-open-error".to_string();
        let target_path = "protected/file.txt";
        let file_content = b"should fail to open";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(target_path);
        msg_raw.push_ulong(file_content.len() as u64);

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_upload(msg);

        let mut server = server;
        let ready = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        // Send chunk data
        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(file_content);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");

        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Failed to open target file for writing");

        // Verify directory still exists and restore permissions for cleanup
        assert!(protected_dir.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&protected_dir, fs::Permissions::from_mode(0o755));
        }
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_large_file() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test12");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1242i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

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

        handle_file_upload(msg);

        let mut server = server;
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

        for chunk in file_content.chunks(64 * 1024) {
            let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
            chunk_msg.push_bytes(chunk);
            server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();
        }

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(5), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_COMPLETE);

        let final_path = Path::new(&working_dir).join(target_path);
        assert_eq!(fs::metadata(final_path).unwrap().len(), 1024 * 1024);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_file_size_mismatch() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test13");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1243i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

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

        handle_file_upload(msg);

        let mut server = server;
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(&[0u8; 500]);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_zero_byte_file() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test14");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1244i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

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

        handle_file_upload(msg);

        let mut server = server;
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_COMPLETE);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_upload_actual_bigger_than_declared() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test15");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1245i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

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

        handle_file_upload(msg);

        let mut server = server;
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await;

        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(&[0u8; 20]);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
    } // end inner()
    inner();
}

#[test_fork::test]
fn test_file_download_pause_resume() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test16");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = vec![0u8; 320 * 1024]; // 320KB (5 chunks of 64KB)
        fs::write(
            fixture.get_temp_path().join("pause_test.bin"),
            &file_content,
        )
        .unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1246i64;
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: working_dir.clone(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job);

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-pause".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("pause_test.bin");

        let msg = Message::from_data(msg_raw.get_data().clone());

        handle_file_download(msg);

        let mut server = server;
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await; // DETAILS
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await; // CHUNK 1

        let pause_msg = Message::new(PAUSE_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
        server.msg_tx.send(pause_msg.get_data().clone()).unwrap();

        // Give time for the pause to propagate through the websocket
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Drain any chunks that were in-flight before the pause took effect
        while let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(50), server.msg_rx.recv()).await
        {}

        // Now that pause is definitely active, no more chunks should arrive
        let res = tokio::time::timeout(Duration::from_millis(500), server.msg_rx.recv()).await;
        assert!(res.is_err(), "Expected no chunks while paused");

        let resume_msg = Message::new(RESUME_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
        server.msg_tx.send(resume_msg.get_data().clone()).unwrap();

        let chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("No chunk after resume")
            .expect("No chunk");
        assert_eq!(chunk.id, FILE_CHUNK);
    } // end inner()
    inner();
}
