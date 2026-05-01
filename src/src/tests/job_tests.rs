use crate::bundle_manager::BundleManager;
use crate::db::{self};
use crate::jobs::{handle_job_submit, MAX_SUBMIT_COUNT};
use crate::messaging::{
    Message, Priority, DB_JOBSTATUS_DELETE_BY_ID_LIST, DB_JOBSTATUS_GET_BY_JOB_ID,
    DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT, DB_JOBSTATUS_SAVE, DB_JOB_DELETE, DB_JOB_GET_BY_ID,
    DB_JOB_GET_BY_JOB_ID, DB_JOB_GET_RUNNING_JOBS, DB_JOB_SAVE, ERROR, SUBMIT_JOB, SYSTEM_SOURCE,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use mockall::predicate::*;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use std::collections::HashMap;

/// Shared state for the mock WS client's in-memory DB.
struct MockDbState {
    jobs: HashMap<i64, job::Model>,
    statuses: HashMap<i64, jobstatus::Model>,
    next_job_id: i64,
    next_status_id: i64,
}

impl Default for MockDbState {
    fn default() -> Self {
        MockDbState {
            jobs: HashMap::new(),
            statuses: HashMap::new(),
            next_job_id: 1,
            next_status_id: 1,
        }
    }
}

/// Helper: create a mock WS client with full DB support using an existing state
fn create_mock_with_db_state(state: &Arc<std::sync::Mutex<MockDbState>>) -> MockWebsocketClient {
    let state_clone = state.clone();
    let mut mock = MockWebsocketClient::new();
    mock.expect_is_server_ready().returning(|| true);
    mock.expect_send_db_request()
        .times(..)
        .returning(move |msg| {
            let mut resp =
                Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
            match msg.id {
                DB_JOBSTATUS_GET_BY_JOB_ID => {
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
                DB_JOBSTATUS_SAVE => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let id = m.pop_ulong() as i64;
                    let what = m.pop_string();
                    let state_val = m.pop_int();
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
                        state: state_val,
                        what,
                    };
                    s.statuses.insert(saved_id, status);
                    resp.push_bool(true);
                    resp.push_ulong(saved_id as u64);
                }
                DB_JOB_GET_BY_ID => {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let id = m.pop_ulong() as i64;
                    let s = state_clone.lock().unwrap();
                    if let Some(job) = s.jobs.get(&id) {
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
                DB_JOB_GET_BY_JOB_ID => {
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
                        job_id: Some(job_id),
                        scheduler_id: Some(scheduler_id),
                        submitting,
                        submitting_count,
                        bundle_hash,
                        working_directory,
                        running,
                        deleting,
                        deleted,
                    };
                    s.jobs.insert(saved_id, saved);
                    resp.push_bool(true);
                    resp.push_ulong(saved_id as u64);
                }
                _ => {
                    resp.push_bool(true);
                    resp.push_uint(0);
                }
            }
            Box::pin(async move { Ok(resp) })
        });
    mock
}

/// Helper: set up a mock WS client with an in-memory DB state.
/// Returns (mock, state) so tests can configure additional expectations before calling `set_websocket_client`.
fn setup_mock_ws() -> (MockWebsocketClient, Arc<std::sync::Mutex<MockDbState>>) {
    crate::websocket::reset_websocket_client_for_test();
    let state = Arc::new(std::sync::Mutex::new(MockDbState::default()));
    let state_clone = state.clone();

    let mut mock = MockWebsocketClient::new();
    mock.expect_is_server_ready().returning(|| true);

    mock.expect_send_db_request().returning(move |msg| {
        let mut resp = Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
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
            id if id == DB_JOB_GET_BY_ID => {
                let mut m = Message::from_data(msg.get_data().clone());
                let id = m.pop_ulong() as i64;
                let s = state_clone.lock().unwrap();
                if let Some(job) = s.jobs.get(&id) {
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
                    resp.push_ulong(0);
                }
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
            DB_JOB_DELETE => {
                let mut m = Message::from_data(msg.get_data().clone());
                let id = m.pop_ulong() as i64;
                state_clone.lock().unwrap().jobs.remove(&id);
                resp.push_bool(true);
            }
            DB_JOB_GET_RUNNING_JOBS => {
                let s = state_clone.lock().unwrap();
                let running: Vec<_> = s.jobs.values().filter(|j| j.running).collect();
                resp.push_bool(true);
                resp.push_uint(running.len() as u32);
                for job in running {
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
                }
            }
            DB_JOBSTATUS_SAVE => {
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
                let saved = jobstatus::Model {
                    id: saved_id,
                    what,
                    state,
                    job_id,
                };
                s.statuses.insert(saved_id, saved.clone());
                resp.push_bool(true);
                resp.push_ulong(saved_id as u64);
            }
            DB_JOBSTATUS_GET_BY_JOB_ID => {
                let mut m = Message::from_data(msg.get_data().clone());
                let job_id = m.pop_ulong() as i64;
                let s = state_clone.lock().unwrap();
                let statuses: Vec<_> = s
                    .statuses
                    .values()
                    .filter(|st| st.job_id == job_id)
                    .collect();
                resp.push_bool(true);
                resp.push_uint(statuses.len() as u32);
                for st in statuses {
                    resp.push_ulong(st.id as u64);
                    resp.push_ulong(st.job_id as u64);
                    resp.push_string(&st.what);
                    resp.push_int(st.state);
                }
            }
            DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT => {
                let mut m = Message::from_data(msg.get_data().clone());
                let job_id = m.pop_ulong() as i64;
                let what = m.pop_string();
                let s = state_clone.lock().unwrap();
                let statuses: Vec<_> = s
                    .statuses
                    .values()
                    .filter(|st| st.job_id == job_id && st.what == what)
                    .collect();
                resp.push_bool(true);
                resp.push_uint(statuses.len() as u32);
                for st in statuses {
                    resp.push_ulong(st.id as u64);
                    resp.push_ulong(st.job_id as u64);
                    resp.push_string(&st.what);
                    resp.push_int(st.state);
                }
            }
            DB_JOBSTATUS_DELETE_BY_ID_LIST => {
                let mut m = Message::from_data(msg.get_data().clone());
                let count = m.pop_uint();
                let mut s = state_clone.lock().unwrap();
                for _ in 0..count {
                    let id = m.pop_ulong() as i64;
                    s.statuses.remove(&id);
                }
                resp.push_bool(true);
            }
            _ => {
                resp.push_bool(true);
                resp.push_ulong(0);
            }
        }
        Box::pin(async move { Ok(resp) })
    });

    (mock, state)
}

pub fn setup_test(_db_name: &str) {
    crate::tests::init_python_global();
}

#[test_fork::test]
fn test_submit_timeout() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let (mut mock_ws, state) = setup_mock_ws();

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

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1234i64;
        let mut job = job::Model {
            id: 1,
            job_id: Some(1234),
            scheduler_id: None,
            submitting: true,
            submitting_count: 0,
            bundle_hash: bundle_hash.clone(),
            working_directory: "/tmp".to_string(),
            running: true,
            deleting: false,
            deleted: false,
        };
        {
            let mut s = state.lock().unwrap();
            s.jobs.insert(1, job.clone());
            s.next_job_id = 2;
        }

        // Try to submit the job many times - each should increment submitting_count
        for count in 1..MAX_SUBMIT_COUNT {
            let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
            msg_raw.push_uint(job_id as u32);
            msg_raw.push_string(&bundle_hash);
            msg_raw.push_string("test params");

            let msg = Message::from_data(msg_raw.get_data().clone());
            handle_job_submit(msg);

            let mut retry = 0;
            while retry < 10 {
                job = state
                    .lock()
                    .unwrap()
                    .jobs
                    .values()
                    .find(|j| j.job_id == Some(job_id))
                    .cloned()
                    .unwrap();
                if job.submitting_count == count {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
                retry += 1;
            }
            assert_eq!(
                job.submitting_count, count,
                "Client never incremented submittingCount at count {count}"
            );
        }

        // Now write a successful submit script
        fixture.write_job_submit(&bundle_hash, "/a/test/working/directory/", "4321");

        let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id as u32);
        msg_raw.push_string(&bundle_hash);
        msg_raw.push_string("test params");

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_job_submit(msg);

        // Wait for the message to be queued
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timed out waiting for queue_message");

        let mut retry = 0;
        while retry < 20 {
            job = state
                .lock()
                .unwrap()
                .jobs
                .values()
                .find(|j| j.job_id == Some(job_id))
                .cloned()
                .unwrap();
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
    inner();
}

#[test_fork::test]
fn test_submit_error_none() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_job_submit_error(&bundle_hash, "None");

        let (mut mock_ws, state) = setup_mock_ws();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // We expect 2 messages on error
        mock_ws
            .expect_queue_message()
            .times(2)
            .returning(move |_, _, _, _| {
                let _ = tx_clone.send(());
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 5678u32;
        let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id);
        msg_raw.push_string(&bundle_hash);
        msg_raw.push_string("test params");

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_job_submit(msg);

        // Wait for messages to be processed (2 calls)
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timed out waiting for first queue_message");
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timed out waiting for second queue_message");

        // Job should have been deleted from state after submit error
        let job_in_state = state
            .lock()
            .unwrap()
            .jobs
            .values()
            .find(|j| j.job_id == Some(i64::from(job_id)))
            .cloned();
        assert!(
            job_in_state.is_none(),
            "Job should be deleted after submit error"
        );
    }
    inner();
}

#[test_fork::test]
fn test_submit_error_zero() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Write submit script that returns 0 (error case)
        fixture.write_job_submit_error(&bundle_hash, "0");

        let (mut mock_ws, state) = setup_mock_ws();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // We expect 2 messages on error
        mock_ws
            .expect_queue_message()
            .times(2)
            .returning(move |_, _, _, _| {
                let _ = tx_clone.send(());
            });

        set_websocket_client(Arc::new(mock_ws));

        let job_id = 1234u32;
        let mut msg_raw = Message::new(SUBMIT_JOB, Priority::Medium, SYSTEM_SOURCE);
        msg_raw.push_uint(job_id);
        msg_raw.push_string(&bundle_hash);
        msg_raw.push_string("test params");

        let msg = Message::from_data(msg_raw.get_data().clone());
        handle_job_submit(msg);

        // Wait for messages to be processed
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timed out waiting for first message");
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timed out waiting for second message");

        // Job should be deleted after submit error
        let job = state
            .lock()
            .unwrap()
            .jobs
            .values()
            .find(|j| j.job_id == Some(i64::from(job_id)))
            .cloned();
        assert!(job.is_none(), "Job should be deleted after submit error");
    }
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_same_status() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let working_dir = TempDir::new().unwrap();
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

        let (mut mock_ws, state) = setup_mock_ws();

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
        let original_id = 1i64;
        let status = jobstatus::Model {
            id: original_id,
            job_id: job.id,
            what: "test_what".to_string(),
            state: RUNNING as i32,
        };
        {
            let mut s = state.lock().unwrap();
            s.jobs.insert(1, job.clone());
            s.statuses.insert(original_id, status);
            s.next_status_id = 2;
        }

        // Should NOT send notification when same status and force_notification=false
        mock_ws.expect_queue_message().times(0);
        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), false).await;

        // Status record should still exist with same state (no update for same status)
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
        assert_eq!(v_status.len(), 1);
        assert_eq!(v_status[0].id, original_id);
        assert_eq!(v_status[0].state, RUNNING as i32);
    }
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_same_status_multiple() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let working_dir = TempDir::new().unwrap();
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Info 1", "what": "what1", "status": 50}, {"info": "Info 2", "what": "what2", "status": 50}], "complete": false}"#);

        let (mut mock_ws, state) = setup_mock_ws();

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
        let original_id1 = 1i64;
        let original_id2 = 2i64;
        {
            let mut s = state.lock().unwrap();
            s.jobs.insert(1, job.clone());
            s.statuses.insert(
                original_id1,
                jobstatus::Model {
                    id: original_id1,
                    job_id: job.id,
                    what: "what1".to_string(),
                    state: RUNNING as i32,
                },
            );
            s.statuses.insert(
                original_id2,
                jobstatus::Model {
                    id: original_id2,
                    job_id: job.id,
                    what: "what2".to_string(),
                    state: RUNNING as i32,
                },
            );
            s.next_status_id = 3;
        }

        // Should NOT send notification when same status and force_notification=false
        mock_ws.expect_queue_message().times(0);
        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), false).await;

        // Status records should still exist with same state
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
        assert_eq!(v_status.len(), 2);
        let ids: Vec<i64> = v_status.iter().map(|s| s.id).collect();
        assert!(ids.contains(&original_id1));
        assert!(ids.contains(&original_id2));
    }
    inner();
}

#[test_fork::test]
fn test_archive_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);

        let temp_dir = tempfile::TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_str().unwrap().to_string();
        fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

        // Set up mock WS with DB support
        let (mock_ws, state) = setup_mock_ws();
        set_websocket_client(Arc::new(mock_ws));

        let job_id = 999i64;
        let job = db::job::Model {
            id: 1,
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
        state.lock().unwrap().jobs.insert(1, job.clone());

        let result = crate::jobs::archive_job(&job).await;
        assert!(result);
        assert!(std::path::Path::new(&working_dir)
            .join("archive.tar.gz")
            .exists());
    }
    inner();
}

#[test_fork::test]
fn test_submit_already_submitted() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
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

        // Set up mock WS with DB support FIRST
        let (mut mock_ws, state) = setup_mock_ws();
        let job_id = 1250i64;
        let job = db::job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: Some(4321),
            submitting: false,
            submitting_count: 0,
            bundle_hash: bundle_hash.clone(),
            working_directory: working_dir.clone(),
            running: true,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job.clone());

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // Expect 2 messages: status update + job completion notification from check_job_status
        mock_ws
            .expect_queue_message()
            .times(2)
            .returning(move |_, _, _, _| {
                let _ = tx_clone.send(());
            });

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

        handle_job_submit(msg);

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
        let job_after = state
            .lock()
            .unwrap()
            .jobs
            .values()
            .find(|j| j.job_id == Some(job_id))
            .cloned()
            .unwrap();
        assert!(!job_after.running, "Job should no longer be running");

        // Verify archive was created
        let archive_path = std::path::Path::new(&working_dir).join("archive.tar.gz");
        assert!(archive_path.exists(), "Archive should be created");
    }
    inner();
}

#[test_fork::test]
fn test_check_all_job_status_stress() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Set up mock first for DB operations
        let state = Arc::new(std::sync::Mutex::new(MockDbState::default()));
        let state_clone = state.clone();

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_is_server_ready().returning(|| true);
        mock_ws
            .expect_send_db_request()
            .times(..)
            .returning(move |msg| {
                // Handle DB requests using shared state
                let mut resp =
                    Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
                match msg.id {
                    DB_JOB_GET_RUNNING_JOBS => {
                        let s = state_clone.lock().unwrap();
                        let running: Vec<_> = s.jobs.values().filter(|j| j.running).collect();
                        resp.push_bool(true);
                        resp.push_uint(running.len() as u32);
                        for job in running {
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
                        }
                    }
                    DB_JOBSTATUS_SAVE => {
                        let mut m = Message::from_data(msg.get_data().clone());
                        let id = m.pop_ulong() as i64;
                        let what = m.pop_string();
                        let state_val = m.pop_int();
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
                            state: state_val,
                            what,
                        };
                        s.statuses.insert(saved_id, status);
                        resp.push_bool(true);
                        resp.push_ulong(saved_id as u64);
                    }
                    DB_JOBSTATUS_GET_BY_JOB_ID => {
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
                        resp.push_ulong(0);
                    }
                }
                Box::pin(async move { Ok(resp) })
            });
        mock_ws
            .expect_queue_message()
            .times(..)
            .returning(|_, _, _, _| {});
        set_websocket_client(Arc::new(mock_ws));

        // Create 100 jobs
        for i in 0..100 {
            let job_id = i64::from(i + 1000);
            let bundle_hash = format!("bundle_{i}");
            let temp_dir = tempfile::TempDir::new().unwrap();
            let working_dir = temp_dir.path().to_str().unwrap().to_string();

            fixture.write_job_submit_check_status(
                &bundle_hash,
                &working_dir,
                &format!("{job_id}"),
                r#"{"complete": true}"#,
            );

            // Create job directly in mock state
            let job = job::Model {
                id: i64::from(i + 1),
                job_id: Some(job_id),
                scheduler_id: Some(4321 + i64::from(i)),
                running: true,
                bundle_hash,
                working_directory: working_dir,
                submitting: false,
                submitting_count: 0,
                deleting: false,
                deleted: false,
            };
            state.lock().unwrap().jobs.insert(i64::from(i + 1), job);
        }

        // Re-create mock with strict expectations for the actual test, using SAME state
        let state_clone2 = state.clone();
        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_is_server_ready().returning(|| true);
        mock_ws
            .expect_send_db_request()
            .times(..)
            .returning(move |msg| {
                let mut resp =
                    Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
                match msg.id {
                    DB_JOB_GET_RUNNING_JOBS => {
                        let s = state_clone2.lock().unwrap();
                        let running: Vec<_> = s.jobs.values().filter(|j| j.running).collect();
                        resp.push_bool(true);
                        resp.push_uint(running.len() as u32);
                        for job in running {
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
                        }
                    }
                    DB_JOBSTATUS_SAVE => {
                        let mut m = Message::from_data(msg.get_data().clone());
                        let id = m.pop_ulong() as i64;
                        let what = m.pop_string();
                        let state_val = m.pop_int();
                        let job_id = m.pop_ulong() as i64;
                        let mut s = state_clone2.lock().unwrap();
                        let saved_id = if id > 0 { id } else { s.next_status_id };
                        if id > 0 {
                            s.next_status_id = std::cmp::max(s.next_status_id, id + 1);
                        } else {
                            s.next_status_id += 1;
                        }
                        let status = jobstatus::Model {
                            id: saved_id,
                            job_id,
                            state: state_val,
                            what,
                        };
                        s.statuses.insert(saved_id, status);
                        resp.push_bool(true);
                        resp.push_ulong(saved_id as u64);
                    }
                    DB_JOBSTATUS_GET_BY_JOB_ID => {
                        let mut m = Message::from_data(msg.get_data().clone());
                        let job_id = m.pop_ulong() as i64;
                        let s = state_clone2.lock().unwrap();
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
                        resp.push_ulong(0);
                    }
                }
                Box::pin(async move { Ok(resp) })
            });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // We expect 100 messages
        mock_ws
            .expect_queue_message()
            .times(100)
            .returning(move |_, _, _, _| {
                let _ = tx_clone.send(());
            });
        set_websocket_client(Arc::new(mock_ws));

        crate::jobs::check_all_jobs_status().await;

        for _ in 0..100 {
            tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("Timeout waiting for stress test status update");
        }
    }
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_new_status() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
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
        let (mut mock_ws, state) = setup_mock_ws();
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: Some(123),
            running: true,
            bundle_hash: bundle_hash.clone(),
            working_directory: working_dir,
            submitting: false,
            submitting_count: 0,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();
        mock_ws.expect_queue_message().returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });
        set_websocket_client(Arc::new(mock_ws));

        crate::jobs::check_job_status(job.clone(), false).await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for status update");

        // Query by internal PK (job.id), not external job_id - status FK references job.id
        let status_after: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
        assert_eq!(status_after.len(), 1);
        assert_eq!(status_after[0].what, "stage1");
        assert_eq!(status_after[0].state, 50); // RUNNING = 50
    }
    inner();
}

#[test_fork::test]
fn test_check_status_no_job_complete() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
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
        let (mut mock_ws, _state) = setup_mock_ws();
        mock_ws.expect_queue_message().returning(|_, _, _, _| {});
        set_websocket_client(Arc::new(mock_ws));

        crate::jobs::check_job_status(job_model, false).await;
    }
    inner();
}

#[test_fork::test]
fn test_delete_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_job_delete(&bundle_hash, "True");

        let job_id = 888i64;
        let (mut mock_ws, state) = setup_mock_ws();
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: bundle_hash.clone(),
            working_directory: String::new(),
            running: false,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();
        mock_ws.expect_queue_message().returning(move |_, _, _, _| {
            let _ = tx_clone.send(());
        });
        set_websocket_client(Arc::new(mock_ws));

        let mut msg_raw = Message::new(
            crate::messaging::DELETE_JOB,
            Priority::Medium,
            SYSTEM_SOURCE,
        );
        msg_raw.push_uint(job_id as u32);
        let msg = Message::from_data(msg_raw.get_data().clone());

        crate::jobs::handle_job_delete(msg);

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for delete msg");

        let job_after = state.lock().unwrap().jobs.get(&1).cloned().unwrap();
        assert!(job_after.deleted);
    }
    inner();
}

#[test_fork::test]
fn test_archive_fail() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);

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
    inner();
}

// ============================================================================
// Job Check Status Tests - ported from test_check_status.cpp
// ============================================================================

use crate::db::{job, jobstatus};
use crate::jobs::check_job_status;
use crate::messaging::{COMPLETED, RUNNING, UPDATE_JOB};
use tempfile::TempDir;

fn setup_check_status_test(
    db_name: &str,
) -> (
    BundleFixture,
    String,
    job::Model,
    TempDir,
    Arc<std::sync::Mutex<MockDbState>>,
) {
    // Reset websocket client FIRST before any setup
    crate::websocket::reset_websocket_client_for_test();
    setup_test(db_name);

    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    // Create a temporary directory for the job's working directory
    let working_dir = TempDir::new().unwrap();

    // Set up mock first for DB operations
    let state = Arc::new(std::sync::Mutex::new(MockDbState::default()));
    let state_clone = state.clone();
    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_is_server_ready().returning(|| true);
    mock_ws
        .expect_send_db_request()
        .times(..)
        .returning(move |msg| {
            let mut resp =
                Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
            if msg.id == DB_JOB_SAVE {
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
                    job_id: Some(job_id),
                    scheduler_id: Some(scheduler_id),
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
            } else {
                resp.push_bool(true);
                resp.push_ulong(0);
            }
            Box::pin(async move { Ok(resp) })
        });
    set_websocket_client(Arc::new(mock_ws));

    // Create a running job in mock state
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

    (fixture, bundle_hash, job, working_dir, state)
}

#[test_fork::test]
fn test_check_status_no_status_not_complete() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::websocket::reset_websocket_client_for_test();
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, state) = setup_check_status_test(&db_name);

        // Write status script that returns empty status, not complete
        fixture.write_job_status(&bundle_hash, r#"{"status": [], "complete": false}"#);

        // Create mock with DB support using the SAME state
        let state_clone = state.clone();
        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_is_server_ready().returning(|| true);
        mock_ws
            .expect_send_db_request()
            .times(..)
            .returning(move |msg| {
                let mut resp =
                    Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
                match msg.id {
                    DB_JOBSTATUS_GET_BY_JOB_ID => {
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
                    DB_JOB_GET_BY_ID => {
                        let mut m = Message::from_data(msg.get_data().clone());
                        let id = m.pop_ulong() as i64;
                        let s = state_clone.lock().unwrap();
                        if let Some(job) = s.jobs.get(&id) {
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
        // No messages expected - no status changes and not complete
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), true).await;

        // No status records should be created
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
        assert_eq!(v_status.len(), 0);

        // Job should still be running
        let job_refreshed = state.lock().unwrap().jobs.get(&job.id).cloned().unwrap();
        assert!(job_refreshed.running);
    }
    inner();
}

#[test_fork::test]
fn test_check_status_no_status_complete() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::websocket::reset_websocket_client_for_test();
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script that returns empty status, complete
        fixture.write_job_status(&bundle_hash, r#"{"status": [], "complete": true}"#);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        state.lock().unwrap().jobs.insert(job.id, job.clone());
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

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
        let job_refreshed = state.lock().unwrap().jobs.get(&job.id).cloned().unwrap();
        assert!(!job_refreshed.running);

        // Archive should have been created
        let archive_path = working_dir.path().join("archive.tar.gz");
        assert!(archive_path.exists());
    }
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_force_notification_duplicates() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script with RUNNING status
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        state.lock().unwrap().jobs.insert(job.id, job.clone());
        {
            let mut s = state.lock().unwrap();
            s.statuses.insert(
                1,
                jobstatus::Model {
                    id: 1,
                    job_id: job.id,
                    what: "test_what".to_string(),
                    state: RUNNING as i32 - 10,
                },
            );
            s.statuses.insert(
                2,
                jobstatus::Model {
                    id: 2,
                    job_id: job.id,
                    what: "test_what".to_string(),
                    state: RUNNING as i32 - 10,
                },
            );
            s.next_status_id = 3;
        }
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), true).await;

        // Duplicates should be deleted, only one status should exist
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
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
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_force_notification() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script with RUNNING status
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

        let original_id = 1i64;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        state.lock().unwrap().jobs.insert(job.id, job.clone());
        {
            let mut s = state.lock().unwrap();
            s.statuses.insert(
                original_id,
                jobstatus::Model {
                    id: original_id,
                    job_id: job.id,
                    what: "test_what".to_string(),
                    state: (RUNNING - 10) as i32,
                },
            );
            s.next_status_id = 2;
        }
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), true).await;

        // Status should be updated (same ID)
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
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
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_force_notification_same_status() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, state) = setup_check_status_test(&db_name);

        // Write status script with RUNNING status
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

        // Create a RUNNING status record (same status) in mock state
        let original_id = {
            let mut s = state.lock().unwrap();
            let status = jobstatus::Model {
                id: s.next_status_id,
                job_id: job.id,
                what: "test_what".to_string(),
                state: RUNNING as i32,
            };
            s.next_status_id += 1;
            let original_id = status.id;
            s.statuses.insert(original_id, status.clone());
            original_id
        };

        // Use the same state from setup_check_status_test
        let state_for_closure = Arc::clone(&state);
        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_is_server_ready().returning(|| true);
        mock_ws
            .expect_send_db_request()
            .times(..)
            .returning(move |msg| {
                let mut resp =
                    Message::new(crate::messaging::DB_RESPONSE, Priority::Medium, "database");
                if msg.id == DB_JOBSTATUS_GET_BY_JOB_ID {
                    let mut m = Message::from_data(msg.get_data().clone());
                    let job_id = m.pop_ulong() as i64;
                    let statuses = {
                        let s = state_for_closure.lock().unwrap();
                        s.statuses
                            .iter()
                            .filter(|(_, st)| st.job_id == job_id)
                            .map(|(_, st)| st.clone())
                            .collect::<Vec<_>>()
                    };
                    resp.push_bool(true);
                    resp.push_uint(statuses.len() as u32);
                    for status in statuses {
                        resp.push_ulong(status.id as u64);
                        resp.push_ulong(status.job_id as u64);
                        resp.push_string(&status.what);
                        resp.push_int(status.state);
                    }
                } else {
                    resp.push_bool(true);
                    resp.push_ulong(0);
                }
                Box::pin(async move { Ok(resp) })
            });
        // Should still send notification because force_notification=true
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();
        mock_ws
            .expect_queue_message()
            .times(..)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), true).await;

        // Status should remain the same
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
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
    inner();
}

#[test_fork::test]
fn test_check_status_new_status_no_existing() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script with RUNNING status
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        state.lock().unwrap().jobs.insert(job.id, job.clone());
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // force_notification=false, but status is new so should still notify
        check_job_status(job.clone(), false).await;

        // One status record should be created
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
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
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_changed_status() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script with RUNNING status
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Some info", "what": "test_what", "status": 50}], "complete": false}"#);

        // Set up mock first for DB operations
        let state = Arc::new(std::sync::Mutex::new(MockDbState::default()));
        state.lock().unwrap().jobs.insert(job.id, job.clone());

        // Create a QUEUED status record using mock with DB support
        let mut mock_ws = create_mock_with_db_state(&state);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        mock_ws
            .expect_queue_message()
            .times(..)
            .returning(move |_, _, _, _| {});
        set_websocket_client(Arc::new(mock_ws));

        let status = jobstatus::Model {
            id: 1,
            job_id: job.id,
            what: "test_what".to_string(),
            state: (RUNNING - 10) as i32, // QUEUED
        };
        state.lock().unwrap().statuses.insert(1, status);

        // Re-create mock with strict expectations, using SAME state
        let mut mock_ws = create_mock_with_db_state(&state);
        let tx_clone = tx.clone();
        mock_ws
            .expect_queue_message()
            .times(1)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // force_notification=false, but status changed so should notify
        check_job_status(job.clone(), false).await;

        // Status record should exist with new state
        let v_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job.id)
            .cloned()
            .collect();
        assert!(v_status.iter().any(|s| s.state == RUNNING as i32));

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
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_error_1() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let temp_dir = tempfile::TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_str().unwrap().to_string();
        // Create a test file that will be archived
        fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

        let job_id = 1234i64;

        // Write status script that returns COMPLETED and ERROR statuses
        // ERROR status (400) should terminate the job
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Step completed", "what": "step1", "status": 500}, {"info": "Error occurred", "what": "step2", "status": 400}], "complete": false}"#);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        let job = job::Model {
            id: 1,
            job_id: Some(job_id),
            scheduler_id: Some(4321),
            running: true,
            bundle_hash: bundle_hash.clone(),
            working_directory: working_dir.clone(),
            submitting: false,
            submitting_count: 0,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(1, job.clone());
        // Expect 3 messages: 2 status updates + 1 completion notification
        mock_ws
            .expect_queue_message()
            .times(3)
            .returning(move |_, _data, _, _| {
                let _ = tx_clone.send(());
            });

        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), false).await;

        // Verify job is no longer running
        let job_after = state.lock().unwrap().jobs.get(&job.id).cloned().unwrap();
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
        let db_status = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            db_status.iter().any(|s| s.state as u32 == ERROR),
            "ERROR status should be recorded"
        );
    }
    inner();
}

#[test_fork::test]
fn test_check_status_exception() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
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
        let job_id_saved = 1i64;
        let original_running = true;
        let original_scheduler_id = Some(4321i64);

        // Create job directly in mock state
        let (mut mock_ws, state) = setup_mock_ws();
        let job = job::Model {
            id: job_id_saved,
            job_id: Some(job_id),
            scheduler_id: Some(4321),
            running: true,
            bundle_hash: bundle_hash.clone(),
            working_directory: "/tmp/test_working_dir".to_string(),
            submitting: false,
            submitting_count: 0,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(job_id_saved, job.clone());

        // No messages should be sent when exception occurs
        mock_ws.expect_queue_message().times(0);

        set_websocket_client(Arc::new(mock_ws));

        // Should not panic
        check_job_status(job.clone(), false).await;

        // Job should be unchanged
        let job_after = state
            .lock()
            .unwrap()
            .jobs
            .get(&job_id_saved)
            .cloned()
            .unwrap();
        assert_eq!(
            job_after.running, original_running,
            "Job running state should be unchanged"
        );
        assert_eq!(
            job_after.scheduler_id, original_scheduler_id,
            "Job scheduler_id should be unchanged"
        );

        // No status records should be created
        let db_status: Vec<_> = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .filter(|s| s.job_id == job_id_saved)
            .cloned()
            .collect();
        assert_eq!(db_status.len(), 0, "No status records should be created");
    }
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_error_2() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        setup_test(&db_name);
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let temp_dir = tempfile::TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_str().unwrap().to_string();
        // Create a test file that will be archived
        fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

        let job_id = 1235i64;
        let job_id_saved = 1i64;

        // WALL_TIME_EXCEEDED status (value > RUNNING and != COMPLETED) terminates the job
        // Using status value 600 which is > RUNNING (50) and != COMPLETED (500)
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Wall time exceeded", "what": "timeout", "status": 600}, {"info": "Step completed", "what": "step1", "status": 500}], "complete": false}"#);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // Set up mock with DB support
        let (mut mock_ws, state) = setup_mock_ws();
        let job = job::Model {
            id: job_id_saved,
            job_id: Some(job_id),
            scheduler_id: Some(4321),
            running: true,
            bundle_hash: bundle_hash.clone(),
            working_directory: working_dir.clone(),
            submitting: false,
            submitting_count: 0,
            deleting: false,
            deleted: false,
        };
        state.lock().unwrap().jobs.insert(job_id_saved, job.clone());

        // Expect 3 messages: 2 status updates + 1 completion notification
        mock_ws
            .expect_queue_message()
            .times(3)
            .returning(move |_, _data, _, _| {
                let _ = tx_clone.send(());
            });

        set_websocket_client(Arc::new(mock_ws));

        check_job_status(job.clone(), false).await;

        // Verify job is no longer running
        let job_after = state
            .lock()
            .unwrap()
            .jobs
            .get(&job_id_saved)
            .cloned()
            .unwrap();
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
        let db_status = state
            .lock()
            .unwrap()
            .statuses
            .values()
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            db_status.iter().any(|s| s.state as u32 == 600),
            "WALL_TIME_EXCEEDED status (600) should be recorded"
        );
    }
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_new_status_multiple() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script with multiple new statuses
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Stage 1 info", "what": "stage1", "status": 50}, {"info": "Stage 2 info", "what": "stage2", "status": 40}], "complete": false}"#);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        state.lock().unwrap().jobs.insert(job.id, job.clone());
        // Expect 2 messages for 2 new statuses
        mock_ws
            .expect_queue_message()
            .times(2)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // force_notification=false, but statuses are new so should notify
        check_job_status(job.clone(), false).await;

        // Two status records should be created
        let v_status: Vec<_> = state.lock().unwrap().statuses.values().cloned().collect();
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
    inner();
}

#[test_fork::test]
fn test_check_status_job_running_changed_status_multiple() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        let db_name = Uuid::new_v4().to_string();
        let (fixture, bundle_hash, job, _working_dir, _state) = setup_check_status_test(&db_name);

        // Write status script with multiple status changes
        fixture.write_job_status(&bundle_hash, r#"{"status": [{"info": "Running info 1", "what": "what1", "status": 50}, {"info": "Running info 2", "what": "what2", "status": 50}], "complete": false}"#);

        // Create two QUEUED status records (will change to RUNNING)
        let original_id1 = 1i64;
        let original_id2 = 2i64;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let (mut mock_ws, state) = setup_mock_ws();
        state.lock().unwrap().jobs.insert(job.id, job.clone());
        {
            let mut s = state.lock().unwrap();
            s.statuses.insert(
                original_id1,
                jobstatus::Model {
                    id: original_id1,
                    job_id: job.id,
                    what: "what1".to_string(),
                    state: (RUNNING - 10) as i32,
                },
            );
            s.statuses.insert(
                original_id2,
                jobstatus::Model {
                    id: original_id2,
                    job_id: job.id,
                    what: "what2".to_string(),
                    state: (RUNNING - 10) as i32,
                },
            );
            s.next_status_id = 3;
        }
        // Expect 2 messages for 2 status changes
        mock_ws
            .expect_queue_message()
            .times(2)
            .returning(move |_, data, _, _| {
                let _ = tx_clone.send(data);
            });

        set_websocket_client(Arc::new(mock_ws));

        // force_notification=false, but statuses changed so should notify
        check_job_status(job.clone(), false).await;

        // Status records should be updated (same IDs)
        let v_status: Vec<_> = state.lock().unwrap().statuses.values().cloned().collect();
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
    inner();
}
