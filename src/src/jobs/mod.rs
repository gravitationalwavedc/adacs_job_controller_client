use crate::bundle_manager::BundleManager;
use crate::config::read_client_config;
use crate::db::{self, job, jobstatus};
use crate::messaging::{
    Message, Priority, CANCELLED, COMPLETED, DELETED, ERROR, RUNNING, SUBMITTED, SYSTEM_SOURCE,
    UPDATE_JOB,
};
use crate::websocket::get_websocket_client;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::path::Path;
use tar::Builder;
use tracing::{error, info, warn};

use tokio::sync::Mutex as TokioMutex;

static SUBMIT_MUTEX: std::sync::LazyLock<TokioMutex<()>> =
    std::sync::LazyLock::new(|| TokioMutex::new(()));

pub const MAX_SUBMIT_COUNT: i32 = 60;

fn get_default_job_details() -> Value {
    json!({
        "cluster": read_client_config()["cluster"]
    })
}

pub fn handle_job_submit(mut msg: Message) {
    let job_id = i64::from(msg.pop_uint());
    let bundle_hash = msg.pop_string();
    let params = msg.pop_string();

    tokio::spawn(async move {
        let mut job_model;

        let mut details = get_default_job_details();
        {
            let _lock = SUBMIT_MUTEX.lock().await;
            job_model = match db::get_or_create_by_job_id(job_id).await {
                Ok(j) => j,
                Err(e) => {
                    error!("DB Error in handle_job_submit: {}", e);
                    return;
                }
            };

            if job_model.submitting {
                job_model.submitting_count += 1;
                if job_model.submitting_count >= MAX_SUBMIT_COUNT {
                    warn!("Job with ID {} took too long to submit - assuming it's failed and trying again...", job_id);
                    job_model.submitting_count = 0;
                    job_model.job_id = Some(0);
                } else {
                    info!("Job with ID {} is being submitted, nothing to do", job_id);
                    let _ = db::save_job(job_model).await;
                    return;
                }
            }

            if job_model.job_id.unwrap_or(0) != 0 && job_model.job_id.unwrap_or(0) == job_id {
                info!(
                    "Job with ID {} has already been submitted, checking status...",
                    job_id
                );
                tokio::spawn(check_job_status(job_model, true));
                return;
            }

            info!("Submitting new job with ui id {}", job_id);
            details["job_id"] = json!(job_id);

            job_model.job_id = Some(job_id);
            job_model.bundle_hash = bundle_hash.clone();
            job_model.submitting = true;
            job_model.working_directory = String::new();
            match db::save_job(job_model).await {
                Ok(j) => job_model = j,
                Err(e) => {
                    error!("Failed to save job during submit: {}", e);
                    return;
                }
            }
        }

        let bm = BundleManager::singleton();
        let working_dir = bm.run_bundle_string("working_directory", &bundle_hash, &details, "");
        job_model.working_directory = working_dir;
        match db::save_job(job_model).await {
            Ok(j) => job_model = j,
            Err(e) => {
                error!("Failed to save job working directory: {}", e);
                return;
            }
        }

        let scheduler_id = bm.run_bundle_uint64("submit", &bundle_hash, &details, &params);

        job_model.scheduler_id = Some(scheduler_id as i64);

        if scheduler_id == 0 {
            error!("Job with UI ID {} could not be submitted", job_id);
            let _ = db::delete_job(job_model.id).await;

            let ws = get_websocket_client();
            let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
            result.push_uint(job_id as u32);
            result.push_string(SYSTEM_SOURCE);
            result.push_uint(ERROR);
            result.push_string("Unable to submit job. Please check the logs as to why.");
            ws.queue_message(
                job_id.to_string(),
                result.get_data().clone(),
                Priority::Medium,
            );

            let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
            result.push_uint(job_id as u32);
            result.push_string("_job_completion_");
            result.push_uint(ERROR);
            result.push_string("Unable to submit job. Please check the logs as to why.");
            ws.queue_message(
                job_id.to_string(),
                result.get_data().clone(),
                Priority::Medium,
            );
        } else {
            job_model.submitting = false;
            job_model.running = true;
            let _ = db::save_job(job_model).await;

            info!(
                "Successfully submitted job with UI ID {}, got scheduler id {}",
                job_id, scheduler_id
            );

            let ws = get_websocket_client();
            let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
            result.push_uint(job_id as u32);
            result.push_string(SYSTEM_SOURCE);
            result.push_uint(SUBMITTED);
            result.push_string("Job submitted successfully");
            ws.queue_message(
                job_id.to_string(),
                result.get_data().clone(),
                Priority::Medium,
            );
        }
    });
}

pub async fn check_job_status(job: job::Model, force_notification: bool) {
    let ws = get_websocket_client();
    if ws.is_connection_closed() || !ws.is_server_ready() {
        info!(
            "Skipping status check for job {} while WebSocket is disconnected",
            job.job_id.unwrap_or(0)
        );
        return;
    }

    let mut details = get_default_job_details();
    details["job_id"] = json!(job.job_id);
    details["scheduler_id"] = json!(job.scheduler_id);

    info!("A. Status");
    let bm = BundleManager::singleton();
    let status_json = bm.run_bundle_json("status", &job.bundle_hash, &details, "");
    info!("A. Status Done");

    if let Some(statuses) = status_json["status"].as_array() {
        for stat in statuses {
            let info = stat["info"].as_str().unwrap_or("");
            let json_status = &stat["status"];
            let what = stat["what"].as_str().unwrap_or("");

            if json_status.is_null() {
                info!("A. jsonStatus was null");
                continue;
            }

            let status = json_status.as_u64().unwrap_or(0) as u32;
            let mut v_status = db::get_job_status_by_job_id_and_what(job.id, what)
                .await
                .unwrap_or_default();

            if v_status.len() > 1 {
                let ids: Vec<i64> = v_status.iter().map(|s| s.id).collect();
                let _ = db::delete_status_by_id_list(ids).await;
                v_status = vec![];
            }

            if force_notification || v_status.is_empty() || status != v_status[0].state as u32 {
                info!("A. Doing update");
                let mut state_item = if v_status.is_empty() {
                    jobstatus::Model {
                        id: 0,
                        job_id: job.id,
                        what: what.to_string(),
                        state: status as i32,
                    }
                } else {
                    v_status[0].clone()
                };

                state_item.what = what.to_string();
                state_item.state = status as i32;
                let _ = db::save_status(state_item).await;

                let ws = get_websocket_client();
                let mut result = Message::new(
                    UPDATE_JOB,
                    Priority::Medium,
                    &job.job_id.unwrap_or(0).to_string(),
                );
                result.push_uint(job.job_id.unwrap_or(0) as u32);
                result.push_string(what);
                result.push_uint(status);
                result.push_string(info);
                ws.queue_message(
                    job.job_id.unwrap_or(0).to_string(),
                    result.get_data().clone(),
                    Priority::Medium,
                );
                info!("A. update message on ws done");
            }
        }
    }

    let v_status = db::get_job_status_by_job_id(job.id)
        .await
        .unwrap_or_default();
    let mut job_error = 0u32;
    for state in &v_status {
        if state.state as u32 > RUNNING && state.state as u32 != COMPLETED {
            job_error = state.state as u32;
        }
    }

    let mut job_complete = true;
    for state in &v_status {
        if state.state as u32 != COMPLETED {
            job_complete = false;
        }
    }

    if job_error != 0 || (status_json["complete"].as_bool().unwrap_or(false) && job_complete) {
        info!("A. Job Complete save");
        let mut job_to_save = job.clone();
        job_to_save.running = false;
        let _ = db::save_job(job_to_save).await;

        info!("A. Archive Job");
        if let Err(e) = archive_job(&job).await {
            warn!("Archive failed for job {}: {}", job.job_id.unwrap_or(0), e);
        }

        let ws = get_websocket_client();
        let mut result = Message::new(
            UPDATE_JOB,
            Priority::Medium,
            &job.job_id.unwrap_or(0).to_string(),
        );
        result.push_uint(job.job_id.unwrap_or(0) as u32);
        result.push_string("_job_completion_");
        result.push_uint(if job_error != 0 { job_error } else { COMPLETED });
        result.push_string("Job has completed");
        ws.queue_message(
            job.job_id.unwrap_or(0).to_string(),
            result.get_data().clone(),
            Priority::Medium,
        );
        info!("A. Send job completion message on ws done");
    }
}

pub async fn check_all_jobs_status() {
    let ws = get_websocket_client();
    if ws.is_connection_closed() || !ws.is_server_ready() {
        info!("Skipping job status check while WebSocket is disconnected");
        return;
    }

    info!("Checking status of running jobs...");
    let jobs = match db::get_running_jobs().await {
        Ok(j) => j,
        Err(e) => {
            error!("Failed to get running jobs: {}", e);
            return;
        }
    };
    info!("There are {} running jobs.", jobs.len());

    let mut handles = vec![];
    for job in jobs {
        handles.push(tokio::spawn(check_job_status(job, false)));
    }

    for handle in handles {
        let _ = handle.await;
    }
}

pub async fn archive_job(job: &job::Model) -> Result<(), String> {
    let working_dir = job.working_directory.clone();
    let job_id = job.job_id.unwrap_or(0);

    info!("Archiving job {}", job_id);

    let result = tokio::task::spawn_blocking(move || {
        let dir = Path::new(&working_dir);
        let archive_path = dir.join("archive.tar.gz");
        archive_dir(dir, &archive_path)
    })
    .await;

    match result {
        Ok(Ok(())) => {
            info!("Archiving job {} completed successfully", job_id);
            Ok(())
        }
        Ok(Err(e)) => {
            error!("Failed to archive job {}: {}", job_id, e);
            Err(e)
        }
        Err(e) => {
            let msg = format!("Failed to spawn archive task for job {job_id}: {e}");
            error!("{}", msg);
            Err(msg)
        }
    }
}

pub fn archive_dir(dir: &Path, archive_path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(archive_path)
        .map_err(|e| format!("Failed to create archive file: {e}"))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);

    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name == "archive.tar.gz" {
            continue;
        }

        builder
            .append_path_with_name(&path, path.strip_prefix(dir).unwrap_or(&path))
            .map_err(|e| format!("Failed to append path {}: {}", path.display(), e))?;
    }

    builder
        .finish()
        .map_err(|e| format!("Failed to finish archive: {e}"))?;
    Ok(())
}

pub fn handle_job_cancel(mut msg: Message) {
    tokio::spawn(async move {
        let job_id = i64::from(msg.pop_uint());

        let Ok(mut job_model) = db::get_or_create_by_job_id(job_id).await else {
            return;
        };

        if job_model.id == 0 || !job_model.running || job_model.submitting {
            error!(
                "Job does not exist ({}), or job is in an invalid state",
                job_id
            );

            if !job_model.submitting {
                let ws = get_websocket_client();
                let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
                result.push_uint(job_id as u32);
                result.push_string("_job_completion_");
                result.push_uint(CANCELLED);
                result.push_string("Job has been cancelled");
                ws.queue_message(
                    job_id.to_string(),
                    result.get_data().clone(),
                    Priority::Medium,
                );
            }
            return;
        }

        // Check if already cancelled before doing status check
        let db_status = db::get_job_status_by_job_id(job_model.id)
            .await
            .unwrap_or_default();
        if db_status.iter().any(|s| s.state as u32 == CANCELLED) {
            info!(
                "Job {} already has CANCELLED status, skipping cancel",
                job_id
            );
            return;
        }

        // Force a status check
        info!("Cancel: About to check status for job {}", job_id);
        check_job_status(job_model.clone(), false).await;
        match db::get_job_by_id(job_model.id).await {
            Ok(Some(j)) => job_model = j,
            Ok(None) => {
                warn!("Cancel: job {} disappeared after status check", job_id);
                return;
            }
            Err(e) => {
                warn!("Cancel: DB error reloading job {}: {}", job_id, e);
                return;
            }
        }
        info!(
            "Cancel: After status check, job.running = {}",
            job_model.running
        );

        if !job_model.running {
            warn!(
                "Job {} is not running so cannot be cancelled, nothing to do.",
                job_id
            );
            return;
        }

        let mut details = get_default_job_details();
        details["job_id"] = json!(job_model.job_id);
        details["scheduler_id"] = json!(job_model.scheduler_id);

        let bm = BundleManager::singleton();
        let cancelled = bm.run_bundle_bool("cancel", &job_model.bundle_hash, &details, "");

        if !cancelled {
            warn!("Job {} could not be cancelled by the bundle.", job_id);
            return;
        }

        match db::get_job_by_id(job_model.id).await {
            Ok(Some(j)) => job_model = j,
            Ok(None) => {
                warn!("Cancel: job {} disappeared after cancel", job_id);
                return;
            }
            Err(e) => {
                warn!("Cancel: DB error after cancel for job {}: {}", job_id, e);
                return;
            }
        }
        check_job_status(job_model.clone(), false).await;

        let db_status = db::get_job_status_by_job_id(job_model.id)
            .await
            .unwrap_or_default();
        if db_status.iter().any(|s| s.state as u32 == CANCELLED) {
            return;
        }

        job_model.running = false;
        let _ = db::save_job(job_model.clone()).await;

        if let Err(e) = archive_job(&job_model).await {
            warn!("Archive failed for job {}: {}", job_id, e);
        }

        let ws = get_websocket_client();
        let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
        result.push_uint(job_id as u32);
        result.push_string("_job_completion_");
        result.push_uint(CANCELLED);
        result.push_string("Job has been cancelled");
        ws.queue_message(
            job_id.to_string(),
            result.get_data().clone(),
            Priority::Medium,
        );
    });
}

pub fn handle_job_delete(mut msg: Message) {
    tokio::spawn(async move {
        let job_id = i64::from(msg.pop_uint());

        let Ok(mut job_model) = db::get_or_create_by_job_id(job_id).await else {
            return;
        };

        if job_model.id == 0 || job_model.running || job_model.submitting || job_model.deleted {
            error!(
                "Job does not exist ({}), is currently running, or has already been deleted.",
                job_id
            );

            if job_model.id == 0 || job_model.deleted {
                let ws = get_websocket_client();
                let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
                result.push_uint(job_id as u32);
                result.push_string("_job_completion_");
                result.push_uint(DELETED);
                result.push_string("Job has been deleted");
                ws.queue_message(
                    job_id.to_string(),
                    result.get_data().clone(),
                    Priority::Medium,
                );
            }
            return;
        }

        if job_model.deleting {
            return;
        }

        job_model.deleting = true;
        match db::save_job(job_model).await {
            Ok(j) => job_model = j,
            Err(e) => {
                error!("Failed to save job for delete: {}", e);
                return;
            }
        }

        let mut details = get_default_job_details();
        details["job_id"] = json!(job_model.job_id);
        details["scheduler_id"] = json!(job_model.scheduler_id);

        let bm = BundleManager::singleton();
        let deleted = bm.run_bundle_bool("delete", &job_model.bundle_hash, &details, "");

        if !deleted {
            job_model.deleting = false;
            let _ = db::save_job(job_model).await;
            warn!("Job {} could not be deleted by the bundle.", job_id);
            return;
        }

        job_model.deleting = false;
        job_model.deleted = true;
        let _ = db::save_job(job_model.clone()).await;

        let ws = get_websocket_client();
        let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
        result.push_uint(job_id as u32);
        result.push_string("_job_completion_");
        result.push_uint(DELETED);
        result.push_string("Job has been deleted");
        ws.queue_message(
            job_id.to_string(),
            result.get_data().clone(),
            Priority::Medium,
        );
    });
}
