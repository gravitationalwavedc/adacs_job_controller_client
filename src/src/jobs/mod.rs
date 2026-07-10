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
use tracing::{debug, error, info, trace, warn};

use tokio::sync::Mutex as TokioMutex;
use walkdir::WalkDir;

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

    debug!(
        "handle_job_submit: job_id={}, bundle_hash={}, params_len={}",
        job_id,
        bundle_hash,
        params.len()
    );

    tokio::spawn(async move {
        let ws = get_websocket_client();
        if ws.is_connection_closed() || !ws.is_server_ready() {
            debug!(
                "Delaying job submit for job {} until server is ready (connected={}, ready={})",
                job_id,
                !ws.is_connection_closed(),
                ws.is_server_ready()
            );
            return;
        }
        debug!("handle_job_submit: WebSocket ready for job {}", job_id);

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
                    debug!("Job with ID {} is being submitted, nothing to do", job_id);
                    let _ = db::save_job(job_model).await;
                    return;
                }
            }

            if job_model.job_id.unwrap_or(0) != 0 && job_model.job_id.unwrap_or(0) == job_id {
                debug!(
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

        debug!(
            "handle_job_submit: submitting job_id={} with bundle_hash={}",
            job_id, bundle_hash
        );
        debug!(
            "handle_job_submit: resolving working directory for ui job id {} via bundle {}",
            job_id, bundle_hash
        );
        let bundle_hash_clone = bundle_hash.clone();
        let details_clone = details.clone();
        let working_dir = tokio::task::spawn_blocking(move || {
            BundleManager::singleton().run_bundle_string(
                "working_directory",
                &bundle_hash_clone,
                &details_clone,
                "",
            )
        })
        .await
        .map_err(|e| {
            error!("handle_job_submit: Python FFI task panicked: {}", e);
            format!("Python FFI task failed: {e}")
        })
        .unwrap_or_else(|e| {
            error!("handle_job_submit: spawn_blocking error: {}", e);
            String::new()
        });
        debug!(
            "handle_job_submit: resolved working directory for ui job id {}: {}",
            job_id, working_dir
        );
        job_model.working_directory = working_dir;
        match db::save_job(job_model).await {
            Ok(j) => job_model = j,
            Err(e) => {
                error!("Failed to save job working directory: {}", e);
                return;
            }
        }

        debug!(
            "handle_job_submit: calling bundle submit for ui job id {} through bundle {}",
            job_id, bundle_hash
        );
        let bundle_hash_clone = bundle_hash.clone();
        let details_clone = details.clone();
        let params_clone = params.clone();
        let scheduler_id = tokio::task::spawn_blocking(move || {
            BundleManager::singleton().run_bundle_uint64(
                "submit",
                &bundle_hash_clone,
                &details_clone,
                &params_clone,
            )
        })
        .await
        .map_err(|e| {
            error!("handle_job_submit: Python FFI task panicked: {}", e);
            format!("Python FFI task failed: {e}")
        })
        .unwrap_or_else(|e| {
            error!("handle_job_submit: spawn_blocking error: {}", e);
            0u64
        });
        debug!(
            "handle_job_submit: bundle submit returned scheduler id {} for ui job id {}",
            scheduler_id, job_id
        );

        job_model.scheduler_id = Some(scheduler_id as i64);

        if scheduler_id == 0 {
            warn!("Job with UI ID {} could not be submitted", job_id);
            let _ = db::delete_job(job_model.id).await;

            let ws = get_websocket_client();
            let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
            result.push_uint(job_id as u32);
            result.push_string(SYSTEM_SOURCE);
            result.push_uint(ERROR);
            result.push_string("Unable to submit job. Please check the logs as to why.");
            debug!(
                "handle_job_submit: queueing ERROR message for job_id={} ({} bytes)",
                job_id,
                result.get_data().len()
            );
            ws.queue_message(
                job_id.to_string(),
                result.get_data().clone(),
                Priority::Medium,
            );
            debug!("handle_job_submit: ERROR message queued");

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
            debug!(
                "handle_job_submit: queueing SUBMITTED message for job_id={} ({} bytes)",
                job_id,
                result.get_data().len()
            );
            ws.queue_message(
                job_id.to_string(),
                result.get_data().clone(),
                Priority::Medium,
            );
            debug!("handle_job_submit: SUBMITTED message queued");
        }
    });
}

pub async fn check_job_status(job: job::Model, force_notification: bool) {
    let job_id = job.job_id.unwrap_or(0);
    debug!(
        "check_job_status: starting for job_id={}, force={}",
        job_id, force_notification
    );
    let ws = get_websocket_client();
    if ws.is_connection_closed() || !ws.is_server_ready() {
        info!(
            "Skipping status check for job {} while WebSocket is disconnected",
            job_id
        );
        return;
    }

    let mut details = get_default_job_details();
    details["job_id"] = json!(job.job_id);
    details["scheduler_id"] = json!(job.scheduler_id);

    debug!(
        "check_job_status: calling bundle status for job_id={}",
        job_id
    );
    let status_start = std::time::Instant::now();
    let bundle_hash_clone = job.bundle_hash.clone();
    let details_clone = details.clone();
    let status_json = tokio::task::spawn_blocking(move || {
        BundleManager::singleton().run_bundle_json("status", &bundle_hash_clone, &details_clone, "")
    })
    .await
    .map_err(|e| {
        error!("check_job_status: Python FFI task panicked: {}", e);
        serde_json::Value::Null
    })
    .unwrap_or_else(|e| {
        error!("check_job_status: spawn_blocking error: {}", e);
        serde_json::Value::Null
    });
    debug!(
        "check_job_status: bundle status call completed in {:?} for job_id={}",
        status_start.elapsed(),
        job_id
    );

    if let Some(statuses) = status_json["status"].as_array() {
        for stat in statuses {
            let info = stat["info"].as_str().unwrap_or("");
            let json_status = &stat["status"];
            let what = stat["what"].as_str().unwrap_or("");

            if json_status.is_null() {
                trace!("check_job_status: jsonStatus was null");
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
                debug!(
                    "check_job_status: updating status for job_id={}, what={}, status={}",
                    job_id, what, status
                );
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
                debug!("check_job_status: saving status to DB");
                let _ = db::save_status(state_item).await;

                let ws = get_websocket_client();
                let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
                result.push_uint(job_id as u32);
                result.push_string(what);
                result.push_uint(status);
                result.push_string(info);
                debug!(
                    "check_job_status: queueing UPDATE_JOB message ({} bytes)",
                    result.get_data().len()
                );
                ws.queue_message(
                    job_id.to_string(),
                    result.get_data().clone(),
                    Priority::Medium,
                );
                debug!("check_job_status: update message queued on ws");
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
        debug!("check_job_status: job complete, saving to DB");
        let mut job_to_save = job.clone();
        job_to_save.running = false;
        let _ = db::save_job(job_to_save).await;

        debug!("check_job_status: archiving job");
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
        trace!("check_job_status: job completion message queued on ws");
    }
}

pub async fn check_all_jobs_status() {
    let ws = get_websocket_client();
    if ws.is_connection_closed() || !ws.is_server_ready() {
        debug!("Skipping job status check while WebSocket is disconnected");
        return;
    }

    trace!("Checking status of running jobs...");
    let get_running_start = std::time::Instant::now();
    let jobs = match db::get_running_jobs().await {
        Ok(j) => j,
        Err(e) => {
            warn!("Failed to get running jobs: {}", e);
            return;
        }
    };
    debug!(
        "There are {} running jobs (get_running_jobs took {:?}).",
        jobs.len(),
        get_running_start.elapsed()
    );

    let mut handles = vec![];
    for job in jobs {
        let job_id = job.job_id.unwrap_or(0);
        debug!(
            "check_all_jobs_status: spawning status check for job_id={}",
            job_id
        );
        handles.push(tokio::spawn(check_job_status(job, false)));
    }

    let wait_start = std::time::Instant::now();
    debug!(
        "check_all_jobs_status: waiting for {} status checks to complete",
        handles.len()
    );
    for (idx, handle) in handles.into_iter().enumerate() {
        let _ = handle.await;
        debug!(
            "check_all_jobs_status: status check {}/{} completed after {:?}",
            idx + 1,
            idx + 1,
            wait_start.elapsed()
        );
    }
    debug!(
        "check_all_jobs_status: all status checks completed in {:?}",
        wait_start.elapsed()
    );
}

pub async fn archive_job(job: &job::Model) -> Result<(), String> {
    let working_dir = job.working_directory.clone();
    let job_id = job.job_id.unwrap_or(0);

    debug!("Archiving job {}", job_id);

    let result = tokio::task::spawn_blocking(move || {
        let dir = Path::new(&working_dir);
        let archive_path = dir.join("archive.tar.gz");
        archive_dir(dir, &archive_path)
    })
    .await;

    match result {
        Ok(Ok(())) => {
            debug!("Archiving job {} completed successfully", job_id);
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

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| e.file_name() != "archive.tar.gz")
        .skip(1)
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();
        let rel = path.strip_prefix(dir).unwrap_or(path);
        let file_type = entry.file_type();
        if file_type.is_dir() {
            builder
                .append_dir(rel, path)
                .map_err(|e| format!("Failed to append dir {}: {}", path.display(), e))?;
        } else {
            builder
                .append_path_with_name(path, rel)
                .map_err(|e| format!("Failed to append path {}: {}", path.display(), e))?;
        }
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
            warn!(
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
            debug!(
                "Job {} already has CANCELLED status, skipping cancel",
                job_id
            );
            return;
        }

        // Force a status check
        debug!("Cancel: About to check status for job {}", job_id);
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
        debug!(
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

        let bundle_hash_clone = job_model.bundle_hash.clone();
        let details_clone = details.clone();
        let cancelled = tokio::task::spawn_blocking(move || {
            BundleManager::singleton().run_bundle_bool(
                "cancel",
                &bundle_hash_clone,
                &details_clone,
                "",
            )
        })
        .await
        .map_err(|e| {
            error!("handle_job_cancel: Python FFI task panicked: {}", e);
            false
        })
        .unwrap_or_else(|e| {
            error!("handle_job_cancel: spawn_blocking error: {}", e);
            false
        });

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
            warn!(
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

        let bundle_hash_clone = job_model.bundle_hash.clone();
        let details_clone = details.clone();
        let deleted = tokio::task::spawn_blocking(move || {
            BundleManager::singleton().run_bundle_bool(
                "delete",
                &bundle_hash_clone,
                &details_clone,
                "",
            )
        })
        .await
        .map_err(|e| {
            error!("handle_job_delete: Python FFI task panicked: {}", e);
            false
        })
        .unwrap_or_else(|e| {
            error!("handle_job_delete: spawn_blocking error: {}", e);
            false
        });

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

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::fs;
    use tar::Archive;
    use tempfile::TempDir;

    #[test]
    fn archive_dir_excludes_archive_tar_gz() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("data.txt"), "job output").unwrap();
        fs::write(dir.join("archive.tar.gz"), "stale archive bytes").unwrap();

        let archive_path = dir.join("output.tar.gz");
        archive_dir(dir, &archive_path).unwrap();

        let file = fs::File::open(&archive_path).unwrap();
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        let entry_names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                entry
                    .unwrap()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert!(
            entry_names.iter().any(|name| name.ends_with("data.txt")),
            "expected data.txt in archive, got: {entry_names:?}"
        );
        assert!(
            !entry_names
                .iter()
                .any(|name| name.ends_with("archive.tar.gz")),
            "archive.tar.gz should be excluded, got: {entry_names:?}"
        );
    }
}
