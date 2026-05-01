pub mod job;
pub mod jobstatus;

use crate::messaging::{
    Message, Priority, DB_JOBSTATUS_DELETE_BY_ID_LIST, DB_JOBSTATUS_GET_BY_JOB_ID,
    DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT, DB_JOBSTATUS_SAVE, DB_JOB_DELETE, DB_JOB_GET_BY_ID,
    DB_JOB_GET_BY_JOB_ID, DB_JOB_GET_RUNNING_JOBS, DB_JOB_SAVE,
};
use crate::websocket::get_websocket_client;

fn parse_response(resp: &Message) -> Message {
    Message::from_data(resp.get_data().clone())
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
        state: resp.pop_int(),
    }
}

pub async fn get_running_jobs() -> Result<Vec<job::Model>, String> {
    let msg = Message::new(DB_JOB_GET_RUNNING_JOBS, Priority::Medium, "database");
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let count = resp.pop_uint() as usize;
    let mut jobs = Vec::with_capacity(count);
    for _ in 0..count {
        jobs.push(parse_job(&mut resp));
    }
    Ok(jobs)
}

pub async fn get_job_by_id(id: i64) -> Result<Option<job::Model>, String> {
    let mut msg = Message::new(DB_JOB_GET_BY_ID, Priority::Medium, "database");
    msg.push_ulong(id as u64);
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let count = resp.pop_uint();
    if count == 0 {
        return Ok(None);
    }
    Ok(Some(parse_job(&mut resp)))
}

pub async fn get_job_by_job_id(job_id_val: i64) -> Result<Option<job::Model>, String> {
    let mut msg = Message::new(DB_JOB_GET_BY_JOB_ID, Priority::Medium, "database");
    msg.push_ulong(job_id_val as u64);
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let count = resp.pop_uint();
    if count == 0 {
        return Ok(None);
    }
    Ok(Some(parse_job(&mut resp)))
}

pub async fn delete_job(id: i64) -> Result<(), String> {
    let mut msg = Message::new(DB_JOB_DELETE, Priority::Medium, "database");
    msg.push_ulong(id as u64);
    get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
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
    let mut msg = Message::new(
        DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT,
        Priority::Medium,
        "database",
    );
    msg.push_ulong(job_id as u64);
    msg.push_string(what);
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let count = resp.pop_uint() as usize;
    let mut statuses = Vec::with_capacity(count);
    for _ in 0..count {
        statuses.push(parse_status(&mut resp));
    }
    Ok(statuses)
}

pub async fn get_job_status_by_job_id(job_id: i64) -> Result<Vec<jobstatus::Model>, String> {
    let mut msg = Message::new(DB_JOBSTATUS_GET_BY_JOB_ID, Priority::Medium, "database");
    msg.push_ulong(job_id as u64);
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let count = resp.pop_uint() as usize;
    let mut statuses = Vec::with_capacity(count);
    for _ in 0..count {
        statuses.push(parse_status(&mut resp));
    }
    Ok(statuses)
}

pub async fn delete_status_by_id_list(ids: Vec<i64>) -> Result<(), String> {
    let mut msg = Message::new(DB_JOBSTATUS_DELETE_BY_ID_LIST, Priority::Medium, "database");
    msg.push_uint(ids.len() as u32);
    for id in ids {
        msg.push_ulong(id as u64);
    }
    get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn save_job(job: job::Model) -> Result<job::Model, String> {
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
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let saved_id = resp.pop_ulong() as i64;
    Ok(job::Model {
        id: saved_id,
        ..job
    })
}

pub async fn save_status(status: jobstatus::Model) -> Result<jobstatus::Model, String> {
    let mut msg = Message::new(DB_JOBSTATUS_SAVE, Priority::Medium, "database");
    msg.push_ulong(status.id as u64);
    msg.push_string(&status.what);
    msg.push_int(status.state);
    msg.push_ulong(status.job_id as u64);
    let raw = get_websocket_client()
        .send_db_request(msg)
        .await
        .map_err(|e| e.to_string())?;
    let mut resp = parse_response(&raw);
    let _success = resp.pop_bool();
    let saved_id = resp.pop_ulong() as i64;
    Ok(jobstatus::Model {
        id: saved_id,
        ..status
    })
}
