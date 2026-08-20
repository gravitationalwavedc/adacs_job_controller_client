use crate::bundle_manager::BundleManager;
use crate::config::TEST_CONFIG;
use crate::db::job;
use crate::files::{
    handle_file_download, handle_file_list, handle_file_upload,
    set_cleanup_failure_observer_for_test, set_final_send_barrier_for_test,
    set_graceful_close_timeout_for_test, set_pre_close_send_barrier_for_test,
    set_server_ready_timeout_for_test, set_transfer_outcome_observer_for_test,
    set_zero_byte_eof_barrier_for_test, TransferOutcome,
};
use crate::messaging::{
    Message, Priority, DB_JOBSTATUS_SAVE, DB_JOB_GET_BY_ID, DB_JOB_GET_BY_JOB_ID, DB_JOB_SAVE,
    DB_RESPONSE, FILE_CHUNK, FILE_DOWNLOAD, FILE_DOWNLOAD_DETAILS, FILE_DOWNLOAD_ERROR, FILE_LIST,
    FILE_LIST_ERROR, FILE_UPLOAD_CHUNK, FILE_UPLOAD_COMPLETE, FILE_UPLOAD_ERROR,
    PAUSE_FILE_CHUNK_STREAM, RESUME_FILE_CHUNK_STREAM, SERVER_READY, SYSTEM_SOURCE, UPLOAD_FILE,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::tests::fixtures::temporary_directory_fixture::TemporaryDirectoryFixture;
use crate::tests::fixtures::websocket_server_fixture::{
    CloseHandshakeBehaviour, ConnectionTermination, ServerReadyBehaviour, WebsocketServerConfig,
    WebsocketServerFixture,
};
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

    mock_ws.expect_is_connection_closed().returning(|| false);
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
                    resp.push_ulong(saved_id as u64);
                }
                id if id == DB_JOB_GET_BY_JOB_ID => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let job_id = m.pop_ulong() as i64;
                    let s = state_clone.lock().unwrap();
                    let found = s.jobs.values().find(|j| j.job_id == Some(job_id));
                    if let Some(job) = found {
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
                        resp.push_uint(0);
                    }
                }
                id if id == DB_JOB_GET_BY_ID => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let id = m.pop_ulong() as i64;
                    let s = state_clone.lock().unwrap();
                    let found = s.jobs.get(&id);
                    if let Some(job) = found {
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
                        resp.push_uint(0);
                    }
                }
                id if id == DB_JOBSTATUS_SAVE => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let _id = m.pop_ulong() as i64;
                    let _job_id = m.pop_ulong() as i64;
                    let _what = m.pop_string();
                    let _state = m.pop_uint() as i32;
                    resp.push_ulong(1);
                }
                _ => {
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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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
        handle_file_list(msg);

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
fn test_get_file_list_database_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_db_err_list");

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_| {
            Box::pin(async move {
                Err(Box::new(std::io::Error::other("db connection failed"))
                    as Box<dyn std::error::Error + Send + Sync>)
            })
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-db-err-list".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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
        handle_file_list(msg);

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_string(),
            "Database error: db connection failed"
        );
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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
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

        handle_file_list(msg);

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
fn test_get_file_list_multiple_concurrent_calls_release_semaphore() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_fl_semaphore");

        let fixture = TemporaryDirectoryFixture::new();
        let list_root = fixture.create_test_directory("list_root");
        let working_dir = list_root.to_str().unwrap().to_string();
        fixture.create_test_file("list_root/file1.txt", "content1");

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-semaphore".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(5)
            .returning(move |_, data, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 7777i64;
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

        // Fire 5 rapid file list requests — proves semaphore releases between calls
        for _ in 0..5 {
            let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
            msg_raw.push_uint(job_id as u32);
            msg_raw.push_string(&test_uuid);
            msg_raw.push_string("some_hash");
            msg_raw.push_string(".");
            msg_raw.push_bool(false);
            let msg = Message::from_data(msg_raw.get_data().clone());
            handle_file_list(msg);
        }

        // Collect all 5 responses
        for i in 0..5 {
            let response = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("Timeout waiting for response {i}"))
                .unwrap_or_else(|| panic!("No response for call {i}"));
            assert_eq!(response.id, FILE_LIST, "call {i} should return FILE_LIST");
            let mut response_msg = response;
            assert_eq!(response_msg.pop_string(), test_uuid);
            let count = response_msg.pop_uint();
            assert_eq!(count, 1, "call {i} should list 1 file");
        }
    }
    inner();
}

#[test_fork::test]
fn test_get_file_list_semaphore_closed_drops_request() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_fl_semaphore_closed");

        crate::files::close_file_list_semaphore_for_test();

        let state = create_mock_state();
        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();
        mock_ws
            .expect_queue_message()
            .times(0)
            .returning(move |_, data, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let test_uuid = "test-uuid-semaphore-closed".to_string();
        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(2234); // Job ID
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string(".");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_file_list(msg);

        // Give the spawned task time to run; a closed semaphore must drop the
        // request without panicking or queuing any response.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            rx.try_recv().is_err(),
            "closed semaphore should drop the request without queuing a response"
        );
    }
    inner();
}

#[test_fork::test]
fn test_get_file_list_unreadable_directory_non_recursive() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_fl_unreadable_non_recursive");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let unreadable = fixture.get_temp_path().join("unreadable");
        fs::create_dir(&unreadable).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        }

        let state = create_mock_state();
        let job_id = 7778i64;
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

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-unreadable-non-recursive".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("unreadable");
        msg_raw.push_bool(false);

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_file_list(msg);

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_uint(),
            0,
            "unreadable directory should yield an empty file list"
        );

        // Restore permissions so TempDir cleanup can remove the directory
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    inner();
}

#[test_fork::test]
fn test_get_file_list_unreadable_directory_recursive() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_fl_unreadable_recursive");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let unreadable = fixture.get_temp_path().join("unreadable");
        fs::create_dir(&unreadable).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        }

        let state = create_mock_state();
        let job_id = 7779i64;
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

        let mut mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "test-uuid-unreadable-recursive".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
                let msg = Message::from_data(data);
                let _ = tx_clone.send(msg);
            });

        set_websocket_client(Arc::new(mock_ws));

        let mut msg_raw = Message::new(FILE_LIST, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("unreadable");
        msg_raw.push_bool(true);

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_file_list(msg);

        let response = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timeout")
            .expect("No response");
        assert_eq!(response.id, FILE_LIST);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), test_uuid);
        assert_eq!(
            response_msg.pop_uint(),
            0,
            "unreadable directory should yield an empty file list"
        );

        // Restore permissions so TempDir cleanup can remove the directory
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
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
            .returning(|_, _, _| {});
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
        let ready = tokio::time::timeout(Duration::from_secs(5), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for SERVER_READY")
            .expect("No ready");
        assert_eq!(ready.id, SERVER_READY);

        let mut chunk_msg = Message::new(FILE_UPLOAD_CHUNK, Priority::Highest, &test_uuid);
        chunk_msg.push_bytes(file_content);
        server.msg_tx.send(chunk_msg.get_data().clone()).unwrap();

        let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &test_uuid);
        server.msg_tx.send(complete_msg.get_data().clone()).unwrap();

        let response = tokio::time::timeout(Duration::from_secs(5), server.msg_rx.recv())
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
fn test_file_upload_database_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test_db_err_upload");

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_| {
            Box::pin(async move {
                Err(Box::new(std::io::Error::other("db connection failed"))
                    as Box<dyn std::error::Error + Send + Sync>)
            })
        });
        set_websocket_client(Arc::new(mock_ws));

        let server = WebsocketServerFixture::new().await;
        set_test_config(server.port);

        let test_uuid = "test-uuid-upload-db-err".to_string();
        let target_path = "test.txt";

        let mut msg_raw = Message::new(UPLOAD_FILE, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_uint(1243); // Job ID
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

        // Error should be sent immediately after SERVER_READY since DB lookup fails
        let response = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for error response")
            .expect("No response");
        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert_eq!(error_msg, "Database error: db connection failed");
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
fn test_file_upload_partial_file_cleanup_on_connection_drop() {
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

        let test_uuid = "test-uuid-upload-conn-drop".to_string();
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

        // Wait for the client to create the partial file before dropping the
        // connection. Without this, the test can pass trivially if the client
        // hasn't started writing yet, leaving the cleanup path uncovered.
        let full_path = fixture.get_temp_path().join(target_path);
        let created_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !full_path.exists() {
            assert!(
                std::time::Instant::now() < created_deadline,
                "Partial file should have been created after upload started"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Drop the connection before FILE_UPLOAD_COMPLETE
        server.stop().await;

        // Verify partial file was cleaned up (deleted)
        let cleaned_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while full_path.exists() {
            assert!(
                std::time::Instant::now() < cleaned_deadline,
                "Partial file should have been cleaned up after connection drop"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
                let ws_url = format!("ws://127.0.0.1:{port}/ws/");

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
fn test_file_upload_create_dir_all_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test36");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        // Create a regular file blocking the parent directory path so
        // create_dir_all fails with ENOTDIR
        let blocker = fixture.get_temp_path().join("blocker");
        fs::write(&blocker, b"not a directory").unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1250i64;
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

        let test_uuid = "test-uuid-create-dir-error".to_string();
        let target_path = "blocker/sub/file.txt";
        let file_content = b"should fail to create parent dir";

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

        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for response")
            .expect("No response");

        assert_eq!(response.id, FILE_UPLOAD_ERROR);
        let mut response_msg = response;
        let error_msg = response_msg.pop_string();
        assert!(
            error_msg.contains("Failed to create parent directory"),
            "Expected create_dir_all error, got: {error_msg}"
        );
        assert_ne!(error_msg, "Failed to open target file for writing");
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

#[cfg(target_os = "linux")]
fn count_open_fds_for_path(path: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
        for entry in entries.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                if target == path {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Count the process's open socket descriptors by inspecting `/proc/self/fd`.
/// This directly measures the client-side symptom in issue #7: one leaked
/// download WebSocket per transfer shows up as one extra `socket:[...]` entry
/// that never returns to baseline.
#[cfg(target_os = "linux")]
fn count_open_socket_fds() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|target| target.to_string_lossy().starts_with("socket:["))
        .count()
}

#[cfg(target_os = "linux")]
#[test_fork::test]
fn test_file_download_pause_then_disconnect_exits_cleanly() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("test35");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_path = fixture.get_temp_path().join("pause_drop_test.bin");
        fs::write(&file_path, vec![0u8; 320 * 1024]).unwrap();
        let canonical_file = fs::canonicalize(&file_path).unwrap();

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

        let test_uuid = "test-uuid-pause-drop".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("pause_drop_test.bin");

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_file_download(msg);

        let mut server = server;
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await; // DETAILS
        let _ = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv()).await; // CHUNK 1

        assert!(
            count_open_fds_for_path(&canonical_file) > 0,
            "download task should hold the file open while streaming"
        );

        let pause_msg = Message::new(PAUSE_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
        server.msg_tx.send(pause_msg.get_data().clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Drain any chunks that were in-flight before the pause took effect
        while let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(50), server.msg_rx.recv()).await
        {}
        let res = tokio::time::timeout(Duration::from_millis(500), server.msg_rx.recv()).await;
        assert!(res.is_err(), "Expected no chunks while paused");

        // Drop the connection while paused (no RESUME). The download loop must
        // wake, observe the dead connection, and release the file handle.
        server.stop().await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut released = false;
        while tokio::time::Instant::now() < deadline {
            if count_open_fds_for_path(&canonical_file) == 0 {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            released,
            "download task leaked the file handle after disconnect while paused"
        );
    } // end inner()
    inner();
}

// ============================================================================
// Issue #7 cleanup and FD-leak regression coverage (task-4).
// Each test asserts against the lifecycle observer and barrier seams that
// Tasks 2 and 3 added. Tests must run serially with --test-threads=1.
// ============================================================================

/// Reset all task-4 test seams. Tests that follow set only the seams they
/// exercise and call this helper at the end so subsequent tests start from a
/// known state.
fn reset_download_test_seams() {
    set_graceful_close_timeout_for_test(None);
    set_final_send_barrier_for_test(None);
    set_zero_byte_eof_barrier_for_test(None);
    set_server_ready_timeout_for_test(None);
    set_pre_close_send_barrier_for_test(None);
    set_transfer_outcome_observer_for_test(None);
    set_cleanup_failure_observer_for_test(None);
}

/// Wait for the lifecycle observer to reach `target` released connections or
/// time out after `budget`. Bounded wall-clock budget keeps the serial suite
/// deterministic even if the supervisor hangs.
async fn wait_for_released(
    observer: &crate::tests::fixtures::websocket_server_fixture::WebsocketLifecycleObserver,
    target: usize,
    budget: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if observer.released_connections() >= target {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

/// Build a 2-chunk file (128 KB total at 64 KB chunks) so the chunk-send
/// failure seam and final-send barrier exercise both Reading and Sending
/// transitions.
fn write_two_chunk_file(path: &std::path::Path) -> Vec<u8> {
    let content = vec![0xAB; 128 * 1024];
    fs::write(path, &content).unwrap();
    content
}

/// Successful download: supervisor sends a Close frame, terminates its
/// supervisor task, and returns the live connection count to baseline.
#[test_fork::test]
fn test_task4_successful_download_closes_and_releases() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_success");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = b"task4 success content";
        fs::write(
            fixture.get_temp_path().join("task4_success.txt"),
            file_content,
        )
        .unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8001i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let server = WebsocketServerFixture::new().await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-success".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("task4_success.txt");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS")
            .expect("No details");
        assert_eq!(details.id, FILE_DOWNLOAD_DETAILS);

        let chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_CHUNK")
            .expect("No chunk");
        assert_eq!(chunk.id, FILE_CHUNK);

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after a successful download"
        );
        assert_eq!(
            observer.live_connections(),
            0,
            "live connection count must return to baseline after success"
        );
        assert!(
            observer.termination() == ConnectionTermination::ClientClose,
            "successful download must record a ClientClose termination, got {:?}",
            observer.termination()
        );
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Each `SERVER_READY` outcome must release the connection. This test loops
/// over every `ServerReadyBehaviour` variant and asserts the lifecycle
/// observer releases the connection for each one.
#[test_fork::test]
fn test_task4_server_ready_branches_release_connection() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_ready_branches");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        fs::write(
            fixture.get_temp_path().join("ready_branch.txt"),
            b"ready branch content",
        )
        .unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8002i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        // Peer EOF before readiness and transport reset both drop the
        // connection before SERVER_READY; the supervisor's readiness timeout
        // covers Withheld. The override shrinks the 10-second production
        // deadline to 50 ms so the Withheld case finishes deterministically.
        set_server_ready_timeout_for_test(Some(Duration::from_millis(50)));

        let variants = [
            (
                ServerReadyBehaviour::Valid,
                ConnectionTermination::StreamEof,
            ),
            (
                ServerReadyBehaviour::InvalidMessageId,
                ConnectionTermination::OtherError,
            ),
            (
                ServerReadyBehaviour::NonBinary,
                ConnectionTermination::OtherError,
            ),
            (
                ServerReadyBehaviour::PeerEof,
                ConnectionTermination::StreamEof,
            ),
            (
                ServerReadyBehaviour::TransportReset,
                ConnectionTermination::ResetWithoutClosingHandshake,
            ),
            (ServerReadyBehaviour::Withheld, ConnectionTermination::None),
        ];

        for (behaviour, _expected_termination) in variants {
            let config = WebsocketServerConfig {
                server_ready: behaviour,
                close_handshake: CloseHandshakeBehaviour::Acknowledge,
                drop_after_n_incoming: None,
            };
            let server = WebsocketServerFixture::with_config(config).await;
            let observer = server.lifecycle();
            set_test_config(server.port);

            let uuid = format!("test-uuid-task4-ready-{behaviour:?}");
            let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
            msg_raw.push_uint(job_id as u32);
            msg_raw.push_string(&uuid);
            msg_raw.push_string("some_hash");
            msg_raw.push_string("ready_branch.txt");

            handle_file_download(Message::from_data(msg_raw.get_data().clone()));

            assert!(
                wait_for_released(&observer, 1, Duration::from_secs(2)).await,
                "{behaviour:?}: supervisor must release the connection"
            );
            assert_eq!(
                observer.live_connections(),
                0,
                "{behaviour:?}: live connection count must return to baseline"
            );
        }
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Job lookup failure cannot strand a connection. The supervisor reports
/// `FILE_DOWNLOAD_ERROR` over the wire and releases the connection.
#[test_fork::test]
fn test_task4_job_lookup_failure_releases_connection() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_job_lookup");

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let server = WebsocketServerFixture::new().await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-job-lookup".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(9999); // Job not in DB
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("missing_job.txt");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");
        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after job lookup failure"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Path validation failure (outside working directory) cannot strand a
/// connection.
#[test_fork::test]
fn test_task4_path_validation_failure_releases_connection() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_path_validation");

        let outside_dir = TemporaryDirectoryFixture::new();
        let outside_working_dir = outside_dir.get_temp_path().to_str().unwrap().to_string();
        let outside_file = outside_dir.get_temp_path().join("outside.txt");
        fs::write(&outside_file, b"outside content").unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8003i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
                id: 1,
                job_id: Some(job_id),
                scheduler_id: None,
                submitting: false,
                submitting_count: 0,
                bundle_hash: String::new(),
                working_directory: outside_working_dir.clone(),
                running: false,
                deleting: false,
                deleted: false,
            },
        );

        let server = WebsocketServerFixture::new().await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-path-val".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        // Symlink-style escape attempt using ../
        msg_raw.push_string("../outside.txt");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");
        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after path validation failure"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// File-open/read failure (requesting a non-existent file inside the working
/// directory) cannot strand a connection.
#[test_fork::test]
fn test_task4_file_not_found_releases_connection() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_file_not_found");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8004i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let server = WebsocketServerFixture::new().await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-missing".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("definitely_not_present.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");
        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection when the file does not exist"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Chunk-send failure never reports COMPLETED. The fixture drops the
/// connection after receiving the first incoming chunk so the supervisor's
/// second chunk send fails; we assert that the supervisor does not emit a
/// second chunk and releases the connection. A successful transfer would
/// emit two `FILE_CHUNK` messages (the file is 128 KB / two 64 KB chunks);
/// observing one or fewer proves the supervisor did not reach `CleanEof`.
#[test_fork::test]
fn test_task4_chunk_send_failure_does_not_complete() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_chunk_send_failure");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let path = fixture.get_temp_path().join("chunk_fail.bin");
        let _content = write_two_chunk_file(&path);

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8005i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: Some(1),
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-chunk-fail".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("chunk_fail.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS")
            .expect("No details");
        assert_eq!(details.id, FILE_DOWNLOAD_DETAILS);

        // Drain FILE_CHUNK messages for up to 200 ms after DETAILS. A
        // successful transfer would emit two FILE_CHUNK messages (the file
        // is two 64 KB chunks). Observing at most one proves the supervisor
        // did not reach CleanEof.
        let mut chunks_received = 0usize;
        while let Ok(Some(msg)) =
            tokio::time::timeout(Duration::from_millis(200), server.msg_rx.recv()).await
        {
            assert_eq!(msg.id, FILE_CHUNK, "unexpected message id");
            chunks_received += 1;
            assert!(
                chunks_received <= 1,
                "supervisor must not emit more than one FILE_CHUNK after \
                 chunk-send failure (received {chunks_received})"
            );
        }
        assert!(
            chunks_received <= 1,
            "supervisor must not complete a 128 KB transfer after the server drops"
        );

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after a chunk-send failure"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Close-send failure after a primary transfer failure does not mask the
/// primary error. We use a deterministic primary error (canonicalise failure
/// for a missing file), park the supervisor before its Close send via the
/// pre-Close-send barrier, reset the fixture peer transport so the Close send
/// fails, and assert the authoritative result is still the primary error, the
/// `FILE_DOWNLOAD_ERROR` was delivered before cleanup, and the connection is
/// released.
#[test_fork::test]
fn test_task4_close_send_failure_does_not_mask_primary_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_close_failure");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        // No file is written: canonicalisation of the requested path fails,
        // selecting a deterministic primary error before any transfer work.

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8006i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let barrier = crate::tests::fixtures::websocket_server_fixture::LifecycleBarrier::new();
        set_pre_close_send_barrier_for_test(Some(barrier.clone()));

        let (outcome_tx, mut outcome_rx) = tokio::sync::mpsc::unbounded_channel();
        set_transfer_outcome_observer_for_test(Some(outcome_tx));
        let (cleanup_tx, mut cleanup_rx) = tokio::sync::mpsc::unbounded_channel();
        set_cleanup_failure_observer_for_test(Some(cleanup_tx));

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: None,
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-close-fail".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("close_fail.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        // The supervisor must deliver the FILE_DOWNLOAD_ERROR before cleanup.
        let error_msg = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No error message");
        assert_eq!(error_msg.id, FILE_DOWNLOAD_ERROR);

        // The authoritative result must be the primary canonicalise failure.
        let outcome = tokio::time::timeout(Duration::from_secs(2), outcome_rx.recv())
            .await
            .expect("supervisor must report an authoritative result")
            .expect("outcome channel closed");
        assert!(
            matches!(outcome, TransferOutcome::PrimaryError(_)),
            "primary canonicalise failure must be selected, got {outcome:?}"
        );

        // Park the supervisor before its Close send, then reset the peer
        // transport so the Close send deterministically fails.
        tokio::time::timeout(Duration::from_secs(2), barrier.wait_until_reached())
            .await
            .expect("supervisor must reach the pre-Close-send barrier");
        server.reset_connection().await;
        // Give the client's reactor time to observe the transport reset so the
        // Close send fails rather than being buffered.
        tokio::time::sleep(Duration::from_millis(50)).await;
        barrier.release();

        // The cleanup failure must be recorded without masking the primary
        // error.
        let cleanup_failure = tokio::time::timeout(Duration::from_secs(2), cleanup_rx.recv())
            .await
            .expect("cleanup must record a failure")
            .expect("cleanup-failure channel closed");
        assert!(
            cleanup_failure.contains("failed to send Close frame"),
            "cleanup failure must be the Close-send failure, got: {cleanup_failure}"
        );

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection even when the Close send fails"
        );
        assert_eq!(observer.live_connections(), 0);
        set_transfer_outcome_observer_for_test(None);
        set_cleanup_failure_observer_for_test(None);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// A Close-send failure after an otherwise successful transfer must preserve
/// the successful transfer result (`CleanEof`) while recording the cleanup
/// failure. We park the supervisor just before its Close send via the
/// pre-Close-send barrier, reset the fixture peer transport (`SO_LINGER=0` RST)
/// so the Close send genuinely fails, release the barrier, and assert the
/// authoritative result was still `CleanEof`, a cleanup failure was recorded,
/// and the connection was released without the fixture ever observing a
/// client Close frame.
#[test_fork::test]
fn test_task4_close_send_failure_after_success_preserves_result() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_close_send_failure_after_success");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let path = fixture.get_temp_path().join("close_after_success.bin");
        let content = vec![0xDD; 32 * 1024];
        fs::write(&path, &content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8014i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let barrier = crate::tests::fixtures::websocket_server_fixture::LifecycleBarrier::new();
        set_pre_close_send_barrier_for_test(Some(barrier.clone()));

        let (outcome_tx, mut outcome_rx) = tokio::sync::mpsc::unbounded_channel();
        set_transfer_outcome_observer_for_test(Some(outcome_tx));
        let (cleanup_tx, mut cleanup_rx) = tokio::sync::mpsc::unbounded_channel();
        set_cleanup_failure_observer_for_test(Some(cleanup_tx));

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: None,
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-close-after-success".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("close_after_success.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");
        let _chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_CHUNK");

        // The transfer must have completed successfully (CleanEof) before the
        // supervisor reaches the pre-Close-send barrier.
        let outcome = tokio::time::timeout(Duration::from_secs(2), outcome_rx.recv())
            .await
            .expect("supervisor must report an authoritative result")
            .expect("outcome channel closed");
        assert!(
            matches!(outcome, TransferOutcome::CleanEof),
            "successful transfer must preserve CleanEof, got {outcome:?}"
        );

        // Park the supervisor before its Close send, then reset the peer
        // transport so the Close send deterministically fails.
        tokio::time::timeout(Duration::from_secs(2), barrier.wait_until_reached())
            .await
            .expect("supervisor must reach the pre-Close-send barrier");
        server.reset_connection().await;
        // Give the client's reactor time to observe the transport reset so the
        // Close send fails rather than being buffered.
        tokio::time::sleep(Duration::from_millis(50)).await;
        barrier.release();

        // The cleanup failure must be recorded (Close-send failure) without
        // replacing the preserved CleanEof result.
        let cleanup_failure = tokio::time::timeout(Duration::from_secs(2), cleanup_rx.recv())
            .await
            .expect("cleanup must record a failure")
            .expect("cleanup-failure channel closed");
        assert!(
            cleanup_failure.contains("failed to send Close frame"),
            "cleanup failure must be the Close-send failure, got: {cleanup_failure}"
        );

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after the Close-send failure"
        );
        assert_eq!(observer.live_connections(), 0);
        assert!(
            !observer.client_close_received(),
            "the fixture must not observe a client Close frame when the Close send failed"
        );
        set_transfer_outcome_observer_for_test(None);
        set_cleanup_failure_observer_for_test(None);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Pause/resume still behaves correctly under the supervisor-driven event
/// loop: after resume, more chunks arrive and the download completes with
/// the connection released.
#[test_fork::test]
fn test_task4_pause_resume_supervisor_event_loop() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_pause_resume");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = vec![0u8; 320 * 1024]; // 320 KB → 5 chunks
        fs::write(
            fixture.get_temp_path().join("pause_resume.bin"),
            &file_content,
        )
        .unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8007i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let server = WebsocketServerFixture::new().await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-pause-resume".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("pause_resume.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");
        let chunk1 = tokio::time::timeout(Duration::from_secs(1), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_CHUNK 1")
            .expect("No chunk 1");
        assert_eq!(chunk1.id, FILE_CHUNK);
        let mut total_chunks = 1usize;

        let pause_msg = Message::new(PAUSE_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
        server.msg_tx.send(pause_msg.get_data().clone()).unwrap();
        // Brief wall-clock delay so the supervisor observes the Pause before
        // we drain remaining chunks; 100 ms is well below the serial-suite
        // timeout and avoids racing the supervisor's first read.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Drain and count any chunks that were in flight before the Pause took
        // effect. These are legitimate (already sent) and must not be lost.
        while let Ok(Some(msg)) =
            tokio::time::timeout(Duration::from_millis(50), server.msg_rx.recv()).await
        {
            assert_eq!(msg.id, FILE_CHUNK, "unexpected message while draining");
            total_chunks += 1;
        }

        let paused_recv =
            tokio::time::timeout(Duration::from_millis(300), server.msg_rx.recv()).await;
        assert!(
            paused_recv.is_err(),
            "no chunks must be received while paused"
        );

        let resume_msg = Message::new(RESUME_FILE_CHUNK_STREAM, Priority::Highest, &test_uuid);
        server.msg_tx.send(resume_msg.get_data().clone()).unwrap();

        // After Resume the remaining chunks must arrive; collect them until the
        // stream ends or a generous timeout expires. The 320 KB file is exactly
        // 5 chunks of 64 KB, so a correct pause/resume transfer must deliver
        // exactly 5 chunks in total with no duplication and no loss.
        let mut resumed = 0usize;
        loop {
            match tokio::time::timeout(Duration::from_secs(3), server.msg_rx.recv()).await {
                Ok(Some(msg)) => {
                    assert_eq!(msg.id, FILE_CHUNK, "unexpected message after resume");
                    resumed += 1;
                }
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            resumed > 0,
            "at least one chunk must arrive after resume"
        );
        total_chunks += resumed;
        assert_eq!(
            total_chunks, 5,
            "pause/resume must deliver exactly 5 chunks (no duplication, no loss); got {total_chunks}"
        );

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(5)).await,
            "supervisor must release the connection after a paused download completes"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Withheld Close acknowledgement triggers forced release using a tiny
/// graceful-close timeout. We override the timeout to 50 ms so the test
/// finishes deterministically without wall-clock sleeps.
#[test_fork::test]
fn test_task4_withheld_close_triggers_forced_release() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_withheld_close");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = b"withheld close";
        fs::write(fixture.get_temp_path().join("withheld.txt"), file_content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8008i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        set_graceful_close_timeout_for_test(Some(Duration::from_millis(50)));

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::WithholdAcknowledgement,
            drop_after_n_incoming: None,
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-withheld".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("withheld.txt");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");
        let _chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_CHUNK");

        // Wait for the supervisor's 50 ms forced-release deadline to fire
        // and for both supervisor-owned halves to be dropped. The fixture
        // server intentionally blocks on stop_signal in
        // WithholdAcknowledgement mode, so we then stop the server to
        // release its task and observe the released connection count.
        tokio::time::sleep(Duration::from_millis(150)).await;
        server.stop().await;

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after the forced-release deadline"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Sustained sequential downloads return the live connection count to
/// baseline after every transfer.
#[test_fork::test]
fn test_task4_sequential_downloads_return_to_baseline() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_sequential");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = b"sequential download content";
        fs::write(fixture.get_temp_path().join("sequential.txt"), file_content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8010i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        // Each iteration uses a fresh fixture because the existing fixture
        // accepts exactly one connection per spawn task.
        for i in 0..3 {
            let server = WebsocketServerFixture::new().await;
            let observer = server.lifecycle();
            set_test_config(server.port);

            let test_uuid = format!("test-uuid-task4-seq-{i}");
            let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
            msg_raw.push_uint(job_id as u32);
            msg_raw.push_string(&test_uuid);
            msg_raw.push_string("some_hash");
            msg_raw.push_string("sequential.txt");

            handle_file_download(Message::from_data(msg_raw.get_data().clone()));

            let mut server = server;
            let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
                .await
                .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");
            let _chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
                .await
                .expect("Timeout waiting for FILE_CHUNK");

            assert!(
                wait_for_released(&observer, 1, Duration::from_secs(2)).await,
                "sequential iteration {i}: supervisor must release the connection"
            );
            assert_eq!(
                observer.live_connections(),
                0,
                "sequential iteration {i}: live connections must return to baseline"
            );
            assert_eq!(
                observer.accepted_connections(),
                1,
                "sequential iteration {i}: one fresh connection must be accepted"
            );
        }
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Sustained sequential downloads must not grow the client's open socket
/// descriptors. This directly exercises issue #7's acceptance criterion
/// ("under a sustained sequence of downloads, the client's open-FD count stays
/// flat"). We warm up the runtime and fixture, record a socket baseline, run a
/// batch of downloads each on a fresh fixture, then assert the socket count
/// returns to baseline (with a small tolerance for lazily-created runtime
/// descriptors).
#[cfg(target_os = "linux")]
#[test_fork::test]
fn test_task4_sustained_downloads_do_not_leak_socket_fds() {
    const ITERATIONS: usize = 30;

    async fn run_one_download(job_id: i64, iteration: usize) {
        let server = WebsocketServerFixture::new().await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = format!("test-uuid-task4-sockfd-{iteration}");
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("sustained.txt");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");
        let _chunk = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_CHUNK");

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "iteration {iteration}: supervisor must release the connection"
        );
        assert_eq!(
            observer.live_connections(),
            0,
            "iteration {iteration}: live connections must return to baseline"
        );
    }

    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_socket_fd_leak");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let file_content = b"sustained download socket fd leak check";
        fs::write(fixture.get_temp_path().join("sustained.txt"), file_content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8011i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        // Warm up: one download so lazy runtime/fixture descriptors exist
        // before we record the baseline.
        run_one_download(job_id, 0).await;

        // Allow sockets to settle before recording the baseline.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let baseline = count_open_socket_fds();

        for i in 1..=ITERATIONS {
            run_one_download(job_id, i).await;
        }

        // Allow sockets to settle after the batch, then assert no growth.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let after = count_open_socket_fds();
        // Small tolerance for lazily-created runtime descriptors; a real leak
        // of one socket per download would exceed this by far.
        assert!(
            after <= baseline + 2,
            "client socket FDs grew from {baseline} to {after} across {ITERATIONS} downloads"
        );

        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Race between the final chunk send and a peer Close. The supervisor must
/// select the peer terminal event, not `CleanEof`, when both are ready at the
/// final-send barrier. We install the barrier and inject Close while the
/// supervisor is parked on it, then release the barrier and assert the
/// authoritative result was a peer terminal event (no COMPLETED log and the
/// connection still releases).
#[test_fork::test]
fn test_task4_final_send_race_peer_close_wins() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_final_send_race");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let path = fixture.get_temp_path().join("race.bin");
        // Single 32 KB chunk — exactly one FILE_CHUNK send completes the file.
        let content = vec![0xCD; 32 * 1024];
        fs::write(&path, &content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8011i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let barrier = crate::tests::fixtures::websocket_server_fixture::LifecycleBarrier::new();
        set_final_send_barrier_for_test(Some(barrier.clone()));

        // Test-only authoritative-result observer: the race assertion must
        // verify the supervisor selected the peer terminal event, not CleanEof.
        let (outcome_tx, mut outcome_rx) = tokio::sync::mpsc::unbounded_channel();
        set_transfer_outcome_observer_for_test(Some(outcome_tx));

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: None,
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-race".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("race.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");

        // Wait for the supervisor to reach the final-send barrier.
        tokio::time::timeout(Duration::from_secs(2), barrier.wait_until_reached())
            .await
            .expect("supervisor must reach the final-send barrier");

        // Inject a genuine WebSocket Close frame while the supervisor is
        // parked on the barrier.
        server.send_peer_close().await;

        // Release the barrier so the supervisor proceeds to Reading and
        // observes the queued peer Close.
        barrier.release();

        // The authoritative result must be the peer terminal event, not
        // CleanEof.
        let outcome = tokio::time::timeout(Duration::from_secs(2), outcome_rx.recv())
            .await
            .expect("supervisor must report an authoritative result")
            .expect("outcome channel closed");
        assert!(
            matches!(outcome, TransferOutcome::PeerTerminal(_)),
            "peer Close must win over CleanEof, got {outcome:?}"
        );

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after the final-send race resolves to peer Close"
        );
        assert_eq!(observer.live_connections(), 0);
        set_transfer_outcome_observer_for_test(None);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Race between the zero-byte EOF read and a peer Close. The supervisor must
/// select the peer terminal event, not `CleanEof`, when a Close is ready at
/// the EOF boundary. We install the zero-byte EOF barrier, queue a peer
/// Close while the supervisor is parked, release the barrier, and assert
/// the connection still releases.
#[test_fork::test]
fn test_task4_zero_byte_eof_race_peer_close_wins() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_eof_race");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let path = fixture.get_temp_path().join("eof_race.bin");
        let content = vec![0xEF; 32 * 1024];
        fs::write(&path, &content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8012i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let barrier = crate::tests::fixtures::websocket_server_fixture::LifecycleBarrier::new();
        set_zero_byte_eof_barrier_for_test(Some(barrier.clone()));

        // Test-only authoritative-result observer: the race assertion must
        // verify the supervisor selected the peer terminal event, not CleanEof.
        let (outcome_tx, mut outcome_rx) = tokio::sync::mpsc::unbounded_channel();
        set_transfer_outcome_observer_for_test(Some(outcome_tx));

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: None,
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-eof-race".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("eof_race.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");

        // Wait for the supervisor to reach the zero-byte EOF barrier.
        tokio::time::timeout(Duration::from_secs(2), barrier.wait_until_reached())
            .await
            .expect("supervisor must reach the zero-byte EOF barrier");

        // Inject a genuine WebSocket Close frame while the supervisor is
        // parked on the EOF barrier.
        server.send_peer_close().await;
        // The supervisor's now_or_never poll is non-blocking, so give the
        // client's reactor time to buffer the Close frame before releasing.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Release the barrier so the supervisor proceeds to the now_or_never
        // poll and observes the queued peer Close.
        barrier.release();

        // The authoritative result must be the peer terminal event, not
        // CleanEof.
        let outcome = tokio::time::timeout(Duration::from_secs(2), outcome_rx.recv())
            .await
            .expect("supervisor must report an authoritative result")
            .expect("outcome channel closed");
        assert!(
            matches!(outcome, TransferOutcome::PeerTerminal(_)),
            "peer Close must win over CleanEof, got {outcome:?}"
        );

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after the zero-byte EOF race resolves to peer Close"
        );
        assert_eq!(observer.live_connections(), 0);
        set_transfer_outcome_observer_for_test(None);
        reset_download_test_seams();
    } // end inner()
    inner();
}

/// Race between the final chunk send and a peer EOF. We make the server
/// drop the connection after receiving both chunks so the supervisor's
/// biased select observes the dead connection instead of a clean EOF.
#[test_fork::test]
fn test_task4_final_send_race_peer_eof_wins() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        setup_test("task4_eof_race_final");

        let fixture = TemporaryDirectoryFixture::new();
        let working_dir = fixture.get_temp_path().to_str().unwrap().to_string();
        let path = fixture.get_temp_path().join("eof_final.bin");
        // Two chunks (128 KB) so drop_after_n_incoming = Some(2) makes the
        // server close the connection after the supervisor has sent both
        // chunks. The supervisor's biased select at the EOF boundary then
        // observes the dropped connection (peer EOF) instead of CleanEof.
        let content = vec![0xEE; 128 * 1024];
        fs::write(&path, &content).unwrap();

        let state = create_mock_state();
        let mock_ws = with_db_support(MockWebsocketClient::new(), &state);
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 8013i64;
        state.lock().unwrap().jobs.insert(
            1,
            job::Model {
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
            },
        );

        let barrier = crate::tests::fixtures::websocket_server_fixture::LifecycleBarrier::new();
        set_final_send_barrier_for_test(Some(barrier.clone()));

        let config = WebsocketServerConfig {
            server_ready: ServerReadyBehaviour::Valid,
            close_handshake: CloseHandshakeBehaviour::Acknowledge,
            drop_after_n_incoming: Some(2),
        };
        let server = WebsocketServerFixture::with_config(config).await;
        let observer = server.lifecycle();
        set_test_config(server.port);

        let test_uuid = "test-uuid-task4-eof-final".to_string();
        let mut msg_raw = Message::new(FILE_DOWNLOAD, Priority::Highest, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&test_uuid);
        msg_raw.push_string("some_hash");
        msg_raw.push_string("eof_final.bin");

        handle_file_download(Message::from_data(msg_raw.get_data().clone()));

        let mut server = server;
        let _details = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_DETAILS");
        let _chunk1 = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_CHUNK 1")
            .expect("No chunk");

        // Wait for the supervisor to reach the final-send barrier after
        // sending chunk 2.
        tokio::time::timeout(Duration::from_secs(2), barrier.wait_until_reached())
            .await
            .expect("supervisor must reach the final-send barrier");

        // Release the barrier; the supervisor proceeds to Reading and the
        // biased select observes the dropped connection (peer EOF).
        barrier.release();

        assert!(
            wait_for_released(&observer, 1, Duration::from_secs(2)).await,
            "supervisor must release the connection after the final-send race resolves to peer EOF"
        );
        assert_eq!(observer.live_connections(), 0);
        reset_download_test_seams();
    } // end inner()
    inner();
}
