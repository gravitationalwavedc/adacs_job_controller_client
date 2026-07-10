pub mod job;
pub mod jobstatus;

use crate::messaging::{
    Message, Priority, DB_JOBSTATUS_DELETE_BY_ID_LIST, DB_JOBSTATUS_GET_BY_JOB_ID,
    DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT, DB_JOBSTATUS_SAVE, DB_JOB_DELETE, DB_JOB_GET_BY_ID,
    DB_JOB_GET_BY_JOB_ID, DB_JOB_GET_RUNNING_JOBS, DB_JOB_SAVE,
};
use crate::websocket::get_websocket_client;
use tracing::{debug, error, trace};

fn parse_response(resp: &Message) -> Message {
    resp.clone_for_payload_reading()
}

fn parse_job(resp: &mut Message) -> job::Model {
    job::Model {
        id: resp.pop_ulong() as i64,
        job_id: {
            let v = resp.pop_ulong() as i64;
            if v != 0 {
                Some(v)
            } else {
                None
            }
        },
        scheduler_id: {
            let v = resp.pop_ulong() as i64;
            if v != 0 {
                Some(v)
            } else {
                None
            }
        },
        submitting: resp.pop_bool(),
        submitting_count: resp.pop_uint() as i32,
        bundle_hash: resp.pop_string(),
        working_directory: resp.pop_string(),
        running: resp.pop_bool(),
        deleting: resp.pop_bool(),
        deleted: resp.pop_bool(),
    }
}

fn parse_status(resp: &mut Message) -> jobstatus::Model {
    jobstatus::Model {
        id: resp.pop_ulong() as i64,
        job_id: resp.pop_ulong() as i64,
        what: resp.pop_string(),
        state: resp.pop_uint() as i32,
    }
}

pub async fn get_running_jobs() -> Result<Vec<job::Model>, String> {
    debug!("DB: get_running_jobs - sending request");
    let msg = Message::new(DB_JOB_GET_RUNNING_JOBS, Priority::Medium, "database");
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!("DB: get_running_jobs - request failed: {}", e);
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let count = resp.pop_uint() as usize;
    debug!(
        "DB: get_running_jobs - received {} jobs in {:?}",
        count, elapsed
    );
    let mut jobs = Vec::with_capacity(count);
    for _ in 0..count {
        jobs.push(parse_job(&mut resp));
    }
    trace!("DB: get_running_jobs - parsed {} job models", jobs.len());
    Ok(jobs)
}

pub async fn get_job_by_id(id: i64) -> Result<Option<job::Model>, String> {
    debug!("DB: get_job_by_id - requesting job id={}", id);
    let mut msg = Message::new(DB_JOB_GET_BY_ID, Priority::Medium, "database");
    msg.push_ulong(id as u64);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!("DB: get_job_by_id - request failed for id={}: {}", id, e);
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let count = resp.pop_uint();
    if count == 0 {
        debug!("DB: get_job_by_id - job id={} not found", id);
        return Ok(None);
    }
    let job = parse_job(&mut resp);
    debug!(
        "DB: get_job_by_id - received job id={} in {:?}",
        id, elapsed
    );
    Ok(Some(job))
}

pub async fn get_job_by_job_id(job_id_val: i64) -> Result<Option<job::Model>, String> {
    debug!(
        "DB: get_job_by_job_id - requesting job_id_val={}",
        job_id_val
    );
    let mut msg = Message::new(DB_JOB_GET_BY_JOB_ID, Priority::Medium, "database");
    msg.push_ulong(job_id_val as u64);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!(
                "DB: get_job_by_job_id - request failed for job_id_val={}: {}",
                job_id_val, e
            );
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let count = resp.pop_uint();
    if count == 0 {
        debug!(
            "DB: get_job_by_job_id - job_id_val={} not found",
            job_id_val
        );
        return Ok(None);
    }
    let job = parse_job(&mut resp);
    debug!("DB: get_job_by_job_id - received job in {:?}", elapsed);
    Ok(Some(job))
}

pub async fn delete_job(id: i64) -> Result<(), String> {
    debug!("DB: delete_job - deleting job id={}", id);
    let mut msg = Message::new(DB_JOB_DELETE, Priority::Medium, "database");
    msg.push_ulong(id as u64);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!("DB: delete_job - request failed for id={}: {}", id, e);
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let _resp = parse_response(&raw);
    debug!("DB: delete_job - completed in {:?}", elapsed);
    Ok(())
}

pub async fn get_or_create_by_job_id(job_id_val: i64) -> Result<job::Model, String> {
    match get_job_by_job_id(job_id_val).await? {
        Some(job) => Ok(job),
        None => Ok(job::Model {
            id: 0,
            job_id: None,
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: String::new(),
            working_directory: String::new(),
            running: false,
            deleted: false,
            deleting: false,
        }),
    }
}

pub async fn get_job_status_by_job_id_and_what(
    job_id: i64,
    what: &str,
) -> Result<Vec<jobstatus::Model>, String> {
    debug!(
        "DB: get_job_status_by_job_id_and_what - job_id={}, what={}",
        job_id, what
    );
    let mut msg = Message::new(
        DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT,
        Priority::Medium,
        "database",
    );
    msg.push_ulong(job_id as u64);
    msg.push_string(what);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!(
                "DB: get_job_status_by_job_id_and_what - request failed: {}",
                e
            );
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let count = resp.pop_uint() as usize;
    debug!(
        "DB: get_job_status_by_job_id_and_what - received {} statuses in {:?}",
        count, elapsed
    );
    let mut statuses = Vec::with_capacity(count);
    for _ in 0..count {
        statuses.push(parse_status(&mut resp));
    }
    trace!(
        "DB: get_job_status_by_job_id_and_what - parsed {} status models",
        statuses.len()
    );
    Ok(statuses)
}

pub async fn get_job_status_by_job_id(job_id: i64) -> Result<Vec<jobstatus::Model>, String> {
    debug!("DB: get_job_status_by_job_id - job_id={}", job_id);
    let mut msg = Message::new(DB_JOBSTATUS_GET_BY_JOB_ID, Priority::Medium, "database");
    msg.push_ulong(job_id as u64);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!("DB: get_job_status_by_job_id - request failed: {}", e);
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let count = resp.pop_uint() as usize;
    debug!(
        "DB: get_job_status_by_job_id - received {} statuses in {:?}",
        count, elapsed
    );
    let mut statuses = Vec::with_capacity(count);
    for _ in 0..count {
        statuses.push(parse_status(&mut resp));
    }
    trace!(
        "DB: get_job_status_by_job_id - parsed {} status models",
        statuses.len()
    );
    Ok(statuses)
}

pub async fn delete_status_by_id_list(ids: Vec<i64>) -> Result<(), String> {
    let mut msg = Message::new(DB_JOBSTATUS_DELETE_BY_ID_LIST, Priority::Medium, "database");
    msg.push_uint(ids.len() as u32);
    for id in ids {
        msg.push_ulong(id as u64);
    }
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let _resp = parse_response(&raw);
    Ok(())
}

pub async fn save_job(job: job::Model) -> Result<job::Model, String> {
    debug!(
        "DB: save_job - saving job id={:?}, job_id={:?}",
        job.id, job.job_id
    );
    let mut msg = Message::new(DB_JOB_SAVE, Priority::Medium, "database");
    msg.push_ulong(job.id as u64);
    msg.push_ulong(job.job_id.unwrap_or(0) as u64);
    msg.push_ulong(job.scheduler_id.unwrap_or(0) as u64);
    msg.push_bool(job.submitting);
    msg.push_uint(job.submitting_count as u32);
    msg.push_string(&job.bundle_hash);
    msg.push_string(&job.working_directory);
    msg.push_bool(job.running);
    msg.push_bool(job.deleting);
    msg.push_bool(job.deleted);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!("DB: save_job - request failed: {}", e);
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let saved_id = resp.pop_ulong() as i64;
    if saved_id == 0 {
        error!("DB: save_job - database returned saved_id=0");
        return Err("Database operation failed to save job".to_string());
    }
    debug!(
        "DB: save_job - saved with new id={} in {:?}",
        saved_id, elapsed
    );
    Ok(job::Model {
        id: saved_id,
        ..job
    })
}

pub async fn save_status(status: jobstatus::Model) -> Result<jobstatus::Model, String> {
    debug!(
        "DB: save_status - saving status id={}, job_id={}, what={}, state={}",
        status.id, status.job_id, status.what, status.state
    );
    let mut msg = Message::new(DB_JOBSTATUS_SAVE, Priority::Medium, "database");
    msg.push_ulong(status.id as u64);
    msg.push_ulong(status.job_id as u64);
    msg.push_string(&status.what);
    msg.push_uint(status.state as u32);
    let send_start = std::time::Instant::now();
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| {
            error!("DB: save_status - request failed: {}", e);
            e.to_string()
        })?;
    let elapsed = send_start.elapsed();
    let mut resp = parse_response(&raw);
    let saved_id = resp.pop_ulong() as i64;
    if saved_id == 0 {
        error!("DB: save_status - database returned saved_id=0");
        return Err("Database operation failed to save job status".to_string());
    }
    debug!(
        "DB: save_status - saved with new id={} in {:?}",
        saved_id, elapsed
    );
    Ok(jobstatus::Model {
        id: saved_id,
        ..status
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::DB_RESPONSE;
    use crate::websocket::{
        reset_websocket_client_for_test, set_websocket_client, MockWebsocketClient,
    };
    use std::sync::{Arc, Mutex};

    fn make_job_response() -> Message {
        let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
        resp.push_uint(1);
        resp.push_ulong(11);
        resp.push_ulong(22);
        resp.push_ulong(33);
        resp.push_bool(true);
        resp.push_uint(4);
        resp.push_string("bundle-hash");
        resp.push_string("/tmp/workdir");
        resp.push_bool(true);
        resp.push_bool(false);
        resp.push_bool(false);
        resp
    }

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn get_running_jobs_sends_header_only_request() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let expected_data = Message::new(DB_JOB_GET_RUNNING_JOBS, Priority::Medium, "database")
            .get_data()
            .clone();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request()
            .times(1)
            .returning(move |message| {
                assert_eq!(message.get_data(), &expected_data);

                let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
                resp.push_uint(0);
                Box::pin(async move { Ok(resp) })
            });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let jobs = rt.block_on(async { get_running_jobs().await }).unwrap();

        assert!(jobs.is_empty());
    }

    #[test]
    fn get_running_jobs_parses_server_payload_without_success_flag() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let resp = make_job_response();
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let jobs = rt.block_on(async { get_running_jobs().await }).unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 11);
        assert_eq!(jobs[0].job_id, Some(22));
        assert_eq!(jobs[0].scheduler_id, Some(33));
        assert_eq!(jobs[0].bundle_hash, "bundle-hash");
    }

    #[test]
    fn get_running_jobs_parses_response_after_request_id_consumed() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let mut wire_resp = Message::new(DB_RESPONSE, Priority::Highest, "system");
            wire_resp.push_uint(7);
            wire_resp.push_uint(1);
            wire_resp.push_ulong(11);
            wire_resp.push_ulong(22);
            wire_resp.push_ulong(33);
            wire_resp.push_bool(true);
            wire_resp.push_uint(4);
            wire_resp.push_string("bundle-hash");
            wire_resp.push_string("/tmp/workdir");
            wire_resp.push_bool(true);
            wire_resp.push_bool(false);
            wire_resp.push_bool(false);

            let mut delivered = Message::from_data(wire_resp.get_data().clone());
            assert_eq!(delivered.pop_uint(), 7);
            Box::pin(async move { Ok(delivered) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let jobs = rt.block_on(async { get_running_jobs().await }).unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 11);
        assert_eq!(jobs[0].job_id, Some(22));
        assert_eq!(jobs[0].scheduler_id, Some(33));
        assert_eq!(jobs[0].bundle_hash, "bundle-hash");
    }

    #[test]
    fn parse_job_reads_deleted_flag() {
        let mut msg = Message::new(DB_RESPONSE, Priority::Highest, "database");
        msg.push_ulong(5);
        msg.push_ulong(0);
        msg.push_ulong(0);
        msg.push_bool(false);
        msg.push_uint(0);
        msg.push_string("hash");
        msg.push_string("/work");
        msg.push_bool(false);
        msg.push_bool(false);
        msg.push_bool(true);

        let mut resp = Message::from_data(msg.get_data().clone());
        let model = parse_job(&mut resp);

        assert_eq!(model.id, 5);
        assert_eq!(model.job_id, None);
        assert_eq!(model.scheduler_id, None);
        assert!(!model.running);
        assert!(!model.deleting);
        assert!(model.deleted);
    }

    #[test]
    fn parse_status_reads_state_as_uint() {
        let mut msg = Message::new(DB_RESPONSE, Priority::Highest, "database");
        msg.push_ulong(99); // id
        msg.push_ulong(42); // job_id
        msg.push_string("scheduler_id"); // what
        msg.push_uint(500); // state (as u32, matching server)

        let mut resp = Message::from_data(msg.get_data().clone());
        // from_data consumes header (source + id), cursor is at payload

        let model = parse_status(&mut resp);
        assert_eq!(model.id, 99);
        assert_eq!(model.job_id, 42);
        assert_eq!(model.what, "scheduler_id");
        assert_eq!(model.state, 500);
    }

    #[test]
    fn parse_status_accepts_empty_what_and_zero_state() {
        let mut msg = Message::new(DB_RESPONSE, Priority::Highest, "database");
        msg.push_ulong(1);
        msg.push_ulong(2);
        msg.push_string("");
        msg.push_uint(0);

        let mut resp = Message::from_data(msg.get_data().clone());
        let model = parse_status(&mut resp);

        assert_eq!(model.id, 1);
        assert_eq!(model.job_id, 2);
        assert_eq!(model.what, "");
        assert_eq!(model.state, 0);
    }

    #[test]
    fn save_status_sends_job_id_before_status_fields() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|message| {
            let mut parsed = Message::from_data(message.get_data().clone());
            assert_eq!(parsed.id, DB_JOBSTATUS_SAVE);
            assert_eq!(parsed.pop_ulong(), 0);
            assert_eq!(parsed.pop_ulong(), 42);
            assert_eq!(parsed.pop_string(), "scheduler_id");
            assert_eq!(parsed.pop_uint(), 500);

            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_ulong(77);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let status = jobstatus::Model {
            id: 0,
            job_id: 42,
            what: "scheduler_id".to_string(),
            state: 500,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let saved = rt.block_on(async { save_status(status).await }).unwrap();
        assert_eq!(saved.id, 77);
    }

    #[test]
    fn get_job_status_by_job_id_and_what_empty_count_returns_empty_vec() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|message| {
            let mut parsed = Message::from_data(message.get_data().clone());
            assert_eq!(parsed.id, DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT);
            assert_eq!(parsed.pop_ulong(), 42);
            assert_eq!(parsed.pop_string(), "scheduler_id");

            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_uint(0);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let statuses = rt
            .block_on(async { get_job_status_by_job_id_and_what(42, "scheduler_id").await })
            .unwrap();

        assert!(statuses.is_empty());
    }
}
