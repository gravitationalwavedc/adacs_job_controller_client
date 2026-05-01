use crate::bundle_manager::BundleManager;
use crate::config::read_client_config;
use crate::db::{self, job, jobstatus};
use crate::messaging::{
    Message, Priority, CANCELLED, COMPLETED, DELETED, ERROR, RUNNING, SUBMITTED, SYSTEM_SOURCE,
    UPDATE_JOB,
};
use crate::websocket::get_websocket_client;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::process::Command;
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
            job_model = db::save_job(job_model).await.expect("Failed to save job");
        }

        let bm = BundleManager::singleton();
        let working_dir = bm.run_bundle_string("working_directory", &bundle_hash, &details, "");
        job_model.working_directory = working_dir;
        job_model = db::save_job(job_model).await.expect("Failed to save job");

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
                Arc::new(|| {}),
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
                Arc::new(|| {}),
            );
        } else {
            job_model.submitting = false;
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
                Arc::new(|| {}),
            );
        }
    });
}

pub async fn check_job_status(job: job::Model, force_notification: bool) {
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
                    Arc::new(|| {}),
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
        archive_job(&job).await;

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
            Arc::new(|| {}),
        );
        info!("A. Send job completion message on ws done");
    }
}

pub async fn check_all_jobs_status() {
    info!("Checking status of running jobs...");
    let jobs = match db::get_running_jobs().await {
        Ok(j) => j,
        Err(e) => {
            error!("Failed to get running jobs: {}", e);
            return;
        }
    };
    info!("There are {} running jobs.", jobs.len()); // Wait, jobs is Vec, should be .len()

    let mut handles = vec![];
    for job in jobs {
        handles.push(tokio::spawn(check_job_status(job, false)));
    }

    for handle in handles {
        let _ = handle.await;
    }
}

pub async fn archive_job(job: &job::Model) -> bool {
    info!("Archiving job {}", job.job_id.unwrap_or(0));
    let output = match Command::new("tar")
        .args(["-cvf", "archive.tar.gz", "--exclude=./archive.tar.gz", "."])
        .current_dir(&job.working_directory)
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            error!("Failed to run tar: {}", e);
            return false;
        }
    };

    let success = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    info!(
        "Archiving job {} completed with code {}\n\nStdout: {}\n\nStderr: {}",
        job.job_id.unwrap_or(0),
        output.status.code().unwrap_or(-1),
        stdout,
        stderr
    );

    success
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
                    Arc::new(|| {}),
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
        job_model = db::get_job_by_id(job_model.id).await.unwrap().unwrap();
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
        details["cluster"] = json!("test_cluster");

        let bm = BundleManager::singleton();
        let cancelled = bm.run_bundle_bool("cancel", &job_model.bundle_hash, &details, "");

        if !cancelled {
            warn!("Job {} could not be cancelled by the bundle.", job_id);
            return;
        }

        job_model = db::get_job_by_id(job_model.id).await.unwrap().unwrap();
        check_job_status(job_model.clone(), false).await;

        let db_status = db::get_job_status_by_job_id(job_model.id)
            .await
            .unwrap_or_default();
        if db_status.iter().any(|s| s.state as u32 == CANCELLED) {
            return;
        }

        job_model.running = false;
        let _ = db::save_job(job_model.clone()).await;

        archive_job(&job_model).await;

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
            Arc::new(|| {}),
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
                    Arc::new(|| {}),
                );
            }
            return;
        }

        if job_model.deleting {
            return;
        }

        job_model.deleting = true;
        job_model = db::save_job(job_model).await.expect("Failed to save job");

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
            Arc::new(|| {}),
        );
    });
}
