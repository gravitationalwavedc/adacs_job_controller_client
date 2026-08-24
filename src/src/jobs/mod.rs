use crate::bundle_manager::{resolve_working_directory, BundleManager};
use crate::config::read_client_config;
use crate::db::{self, job, jobstatus};
use crate::messaging::{
    Message, Priority, CANCELLED, COMPLETED, DELETED, ERROR, JOB_COMPLETION_SOURCE, RUNNING,
    SUBMITTED, SYSTEM_SOURCE, UPDATE_JOB,
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

pub const ARCHIVE_FILE_NAME: &str = "archive.tar.gz";

fn get_default_job_details() -> Value {
    json!({
        "cluster": read_client_config()["cluster"]
    })
}

fn queue_job_update(job_id: i64, source: &str, status: u32, message: &str) {
    let ws = get_websocket_client();
    let mut result = Message::new(UPDATE_JOB, Priority::Medium, &job_id.to_string());
    result.push_uint(job_id as u32);
    result.push_string(source);
    result.push_uint(status);
    result.push_string(message);
    ws.queue_message(
        job_id.to_string(),
        result.get_data().clone(),
        Priority::Medium,
    );
}

fn get_job_details(job: &job::Model) -> Value {
    let mut details = get_default_job_details();
    details["job_id"] = json!(job.job_id);
    details["scheduler_id"] = json!(job.scheduler_id);
    details
}

async fn save_job_or_abort(job_model: job::Model, context: &str) -> Option<job::Model> {
    match db::save_job(job_model).await {
        Ok(j) => Some(j),
        Err(e) => {
            error!("Failed to save job {}: {}", context, e);
            None
        }
    }
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
            job_model = match get_or_create_job_or_abort(job_id, "handle_job_submit").await {
                Some(j) => j,
                None => return,
            };

            if job_model.submitting {
                job_model.submitting_count += 1;
                if job_model.submitting_count >= MAX_SUBMIT_COUNT {
                    warn!("Job with ID {} took too long to submit - assuming it's failed and trying again...", job_id);
                    job_model.submitting_count = 0;
                    job_model.job_id = Some(0);
                } else {
                    debug!("Job with ID {} is being submitted, nothing to do", job_id);
                    if let Err(e) = db::save_job(job_model).await {
                        error!("Failed to save job {} during submit: {}", job_id, e);
                    }
                    return;
                }
            }

            if job_id != 0 && job_model.job_id == Some(job_id) {
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
            job_model = match save_job_or_abort(job_model, "during submit").await {
                Some(j) => j,
                None => return,
            };
        }

        debug!(
            "handle_job_submit: submitting job_id={} with bundle_hash={}",
            job_id, bundle_hash
        );
        debug!(
            "handle_job_submit: resolving working directory for ui job id {} via bundle {}",
            job_id, bundle_hash
        );
        let working_dir =
            resolve_working_directory(&bundle_hash, details.clone(), "", "handle_job_submit").await;
        debug!(
            "handle_job_submit: resolved working directory for ui job id {}: {}",
            job_id, working_dir
        );
        job_model.working_directory = working_dir;
        job_model = match save_job_or_abort(job_model, "working directory").await {
            Some(j) => j,
            None => return,
        };

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
            if let Err(e) = db::delete_job(job_model.id).await {
                error!(
                    "handle_job_submit: failed to delete job {} after submit failure: {}",
                    job_id, e
                );
            }

            queue_job_update(
                job_id,
                SYSTEM_SOURCE,
                ERROR,
                "Unable to submit job. Please check the logs as to why.",
            );
            queue_job_update(
                job_id,
                JOB_COMPLETION_SOURCE,
                ERROR,
                "Unable to submit job. Please check the logs as to why.",
            );
        } else {
            job_model.submitting = false;
            job_model.running = true;
            if let Err(e) = db::save_job(job_model).await {
                error!("Failed to save job {} after submit: {}", job_id, e);
            }

            info!(
                "Successfully submitted job with UI ID {}, got scheduler id {}",
                job_id, scheduler_id
            );

            queue_job_update(
                job_id,
                SYSTEM_SOURCE,
                SUBMITTED,
                "Job submitted successfully",
            );
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

    let details = get_job_details(&job);

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
                .unwrap_or_else(|e| {
                    error!(
                        "check_job_status: failed to read status for job_id={}, what={}: {}",
                        job_id, what, e
                    );
                    Vec::new()
                });

            if v_status.len() > 1 {
                let ids: Vec<i64> = v_status.iter().map(|s| s.id).collect();
                if let Err(e) = db::delete_status_by_id_list(ids).await {
                    error!(
                        "check_job_status: failed to delete duplicate statuses for job_id={}: {}",
                        job_id, e
                    );
                }
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
                if let Err(e) = db::save_status(state_item).await {
                    error!("Failed to save status for job {}: {}", job_id, e);
                }

                queue_job_update(job_id, what, status, info);
                debug!("check_job_status: update message queued on ws");
            }
        }
    }

    let v_status = db::get_job_status_by_job_id(job.id)
        .await
        .unwrap_or_else(|e| {
            error!(
                "check_job_status: failed to read status for job_id={}: {}",
                job_id, e
            );
            Vec::new()
        });
    let job_error = v_status
        .iter()
        .filter(|state| state.state as u32 > RUNNING && state.state as u32 != COMPLETED)
        .map(|state| state.state as u32)
        .next_back()
        .unwrap_or(0);

    let job_complete = v_status.iter().all(|state| state.state as u32 == COMPLETED);

    if job_error != 0 || (status_json["complete"].as_bool().unwrap_or(false) && job_complete) {
        debug!("check_job_status: job complete, saving to DB");
        let mut job_to_save = job.clone();
        job_to_save.running = false;
        if let Err(e) = db::save_job(job_to_save).await {
            error!("Failed to save job {} as complete: {}", job_id, e);
        }

        debug!("check_job_status: archiving job");
        if let Err(e) = archive_job(&job).await {
            warn!("Archive failed for job {}: {}", job.job_id.unwrap_or(0), e);
        }

        queue_job_update(
            job_id,
            JOB_COMPLETION_SOURCE,
            if job_error != 0 { job_error } else { COMPLETED },
            "Job has completed",
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
    let total = handles.len();
    for (idx, handle) in handles.into_iter().enumerate() {
        if let Err(e) = handle.await {
            warn!(
                "check_all_jobs_status: status check {}/{} panicked: {}",
                idx + 1,
                total,
                e
            );
        }
        debug!(
            "check_all_jobs_status: status check {}/{} completed after {:?}",
            idx + 1,
            total,
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
        let archive_path = dir.join(ARCHIVE_FILE_NAME);
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
        .filter_entry(|e| e.file_name() != ARCHIVE_FILE_NAME)
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

async fn reload_job_or_abort(job_model: &mut job::Model, job_id: i64, context: &str) -> bool {
    match db::get_job_by_id(job_model.id).await {
        Ok(Some(j)) => {
            *job_model = j;
            false
        }
        Ok(None) => {
            warn!("Cancel: job {} disappeared after {}", job_id, context);
            true
        }
        Err(e) => {
            warn!(
                "Cancel: DB error reloading job {} after {}: {}",
                job_id, context, e
            );
            true
        }
    }
}

async fn run_bundle_bool_for_job(
    context: &str,
    function_name: &str,
    bundle_hash: &str,
    details: &Value,
) -> bool {
    let function_name = function_name.to_string();
    let bundle_hash_clone = bundle_hash.to_string();
    let details_clone = details.clone();
    tokio::task::spawn_blocking(move || {
        BundleManager::singleton().run_bundle_bool(
            &function_name,
            &bundle_hash_clone,
            &details_clone,
            "",
        )
    })
    .await
    .unwrap_or_else(|e| {
        error!("{}: spawn_blocking error: {}", context, e);
        false
    })
}

async fn get_or_create_job_or_abort(job_id: i64, context: &str) -> Option<job::Model> {
    match db::get_or_create_by_job_id(job_id).await {
        Ok(j) => Some(j),
        Err(e) => {
            error!("DB Error in {}: {}", context, e);
            None
        }
    }
}

pub fn handle_job_cancel(mut msg: Message) {
    tokio::spawn(async move {
        let job_id = i64::from(msg.pop_uint());

        let Some(mut job_model) = get_or_create_job_or_abort(job_id, "handle_job_cancel").await
        else {
            return;
        };

        if job_model.id == 0 || !job_model.running || job_model.submitting {
            warn!(
                "Job does not exist ({}), or job is in an invalid state",
                job_id
            );

            if !job_model.submitting {
                queue_job_update(
                    job_id,
                    JOB_COMPLETION_SOURCE,
                    CANCELLED,
                    "Job has been cancelled",
                );
            }
            return;
        }

        // Check if already cancelled before doing status check
        let db_status = match db::get_job_status_by_job_id(job_model.id).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Cancel: failed to read status for job {}: {}", job_id, e);
                vec![]
            }
        };
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
        if reload_job_or_abort(&mut job_model, job_id, "status check").await {
            return;
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

        let details = get_job_details(&job_model);

        let cancelled = run_bundle_bool_for_job(
            "handle_job_cancel",
            "cancel",
            &job_model.bundle_hash,
            &details,
        )
        .await;

        if !cancelled {
            warn!("Job {} could not be cancelled by the bundle.", job_id);
            return;
        }

        if reload_job_or_abort(&mut job_model, job_id, "cancel").await {
            return;
        }
        check_job_status(job_model.clone(), false).await;

        let db_status = match db::get_job_status_by_job_id(job_model.id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Cancel: failed to read status after cancel for job {}: {}",
                    job_id, e
                );
                vec![]
            }
        };
        if db_status.iter().any(|s| s.state as u32 == CANCELLED) {
            return;
        }

        job_model.running = false;
        if let Err(e) = db::save_job(job_model.clone()).await {
            error!("Failed to save job {} after cancel: {}", job_id, e);
        }

        if let Err(e) = archive_job(&job_model).await {
            warn!("Archive failed for job {}: {}", job_id, e);
        }

        queue_job_update(
            job_id,
            JOB_COMPLETION_SOURCE,
            CANCELLED,
            "Job has been cancelled",
        );
    });
}

pub fn handle_job_delete(mut msg: Message) {
    tokio::spawn(async move {
        let job_id = i64::from(msg.pop_uint());

        let Some(mut job_model) = get_or_create_job_or_abort(job_id, "handle_job_delete").await
        else {
            return;
        };

        if job_model.id == 0 || job_model.running || job_model.submitting || job_model.deleted {
            warn!(
                "Job does not exist ({}), is currently running, or has already been deleted.",
                job_id
            );

            if job_model.id == 0 || job_model.deleted {
                queue_job_update(
                    job_id,
                    JOB_COMPLETION_SOURCE,
                    DELETED,
                    "Job has been deleted",
                );
            }
            return;
        }

        if job_model.deleting {
            return;
        }

        job_model.deleting = true;
        job_model = match save_job_or_abort(job_model, "for delete").await {
            Some(j) => j,
            None => return,
        };

        let details = get_job_details(&job_model);

        let deleted = run_bundle_bool_for_job(
            "handle_job_delete",
            "delete",
            &job_model.bundle_hash,
            &details,
        )
        .await;

        if !deleted {
            job_model.deleting = false;
            if let Err(e) = db::save_job(job_model).await {
                error!("Failed to save job {} after failed delete: {}", job_id, e);
            }
            warn!("Job {} could not be deleted by the bundle.", job_id);
            return;
        }

        job_model.deleting = false;
        job_model.deleted = true;
        if let Err(e) = db::save_job(job_model.clone()).await {
            error!("Failed to save job {} as deleted: {}", job_id, e);
        }

        queue_job_update(
            job_id,
            JOB_COMPLETION_SOURCE,
            DELETED,
            "Job has been deleted",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::DB_RESPONSE;
    use crate::websocket::{
        reset_websocket_client_for_test, set_websocket_client, MockWebsocketClient,
    };
    use flate2::read::GzDecoder;
    use mockall::predicate::{always, eq};
    use serde_json::json;
    use std::sync::Arc;
    use tar::Archive;

    #[test]
    #[serial_test::serial]
    fn get_default_job_details_includes_cluster_from_config() {
        let saved = crate::config::TEST_CONFIG.lock().unwrap().clone();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(json!({
            "cluster": "ozstar",
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://127.0.0.1:0/ws/",
        }));

        let details = get_default_job_details();
        assert_eq!(details["cluster"], "ozstar");

        *crate::config::TEST_CONFIG.lock().unwrap() = saved;
    }

    #[test]
    #[serial_test::serial]
    fn get_job_details_includes_cluster_job_id_and_scheduler_id() {
        let saved = crate::config::TEST_CONFIG.lock().unwrap().clone();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(json!({
            "cluster": "ozstar",
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://127.0.0.1:0/ws/",
        }));

        let details = get_job_details(&make_job_model());
        assert_eq!(details["cluster"], "ozstar");
        assert_eq!(details["job_id"], 1234);
        assert_eq!(details["scheduler_id"], 4321);

        *crate::config::TEST_CONFIG.lock().unwrap() = saved;
    }

    #[test]
    #[serial_test::serial]
    fn get_job_details_serializes_none_ids_as_null() {
        let saved = crate::config::TEST_CONFIG.lock().unwrap().clone();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(json!({
            "cluster": "ozstar",
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://127.0.0.1:0/ws/",
        }));

        let mut job_model = make_job_model();
        job_model.job_id = None;
        job_model.scheduler_id = None;

        let details = get_job_details(&job_model);
        assert!(details["job_id"].is_null());
        assert!(details["scheduler_id"].is_null());

        *crate::config::TEST_CONFIG.lock().unwrap() = saved;
    }

    fn make_job_model() -> job::Model {
        job::Model {
            id: 7,
            job_id: Some(1234),
            scheduler_id: Some(4321),
            submitting: false,
            submitting_count: 0,
            bundle_hash: "old-hash".to_string(),
            working_directory: "/old".to_string(),
            running: true,
            deleting: false,
            deleted: false,
        }
    }

    #[test]
    #[serial_test::serial]
    fn reload_job_or_abort_reloads_job_when_found() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_uint(1);
            resp.push_ulong(99);
            resp.push_ulong(1234);
            resp.push_ulong(4321);
            resp.push_bool(false);
            resp.push_uint(0);
            resp.push_string("bundle-hash");
            resp.push_string("/tmp/workdir");
            resp.push_bool(false);
            resp.push_bool(false);
            resp.push_bool(false);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let mut job_model = make_job_model();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let aborted = rt.block_on(async { reload_job_or_abort(&mut job_model, 7, "cancel").await });

        assert!(!aborted);
        assert_eq!(job_model.id, 99);
        assert_eq!(job_model.job_id, Some(1234));
        assert_eq!(job_model.scheduler_id, Some(4321));
        assert_eq!(job_model.bundle_hash, "bundle-hash");
        assert_eq!(job_model.working_directory, "/tmp/workdir");
        assert!(!job_model.running);
    }

    #[test]
    #[serial_test::serial]
    fn reload_job_or_abort_aborts_when_job_disappeared() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_uint(0);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let mut job_model = make_job_model();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let aborted = rt.block_on(async { reload_job_or_abort(&mut job_model, 7, "cancel").await });

        assert!(aborted);
        assert_eq!(job_model.id, 7);
        assert_eq!(job_model.job_id, Some(1234));
        assert!(job_model.running);
    }

    #[test]
    #[serial_test::serial]
    fn reload_job_or_abort_aborts_on_db_error() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            Box::pin(async move {
                Err(Box::new(std::io::Error::other("db connection failed"))
                    as Box<dyn std::error::Error + Send + Sync>)
            })
        });
        set_websocket_client(Arc::new(mock));

        let mut job_model = make_job_model();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let aborted = rt.block_on(async { reload_job_or_abort(&mut job_model, 7, "cancel").await });

        assert!(aborted);
        assert_eq!(job_model.id, 7);
        assert_eq!(job_model.job_id, Some(1234));
        assert!(job_model.running);
    }

    #[test]
    #[serial_test::serial]
    fn save_job_or_abort_returns_saved_job_on_success() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_ulong(99);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let saved = rt.block_on(async { save_job_or_abort(make_job_model(), "test").await });

        let saved = saved.expect("save_job_or_abort should return Some on success");
        assert_eq!(saved.id, 99);
        assert_eq!(saved.job_id, Some(1234));
        assert_eq!(saved.scheduler_id, Some(4321));
        assert_eq!(saved.bundle_hash, "old-hash");
        assert_eq!(saved.working_directory, "/old");
    }

    #[test]
    #[serial_test::serial]
    fn save_job_or_abort_returns_none_on_db_error() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            Box::pin(async move {
                Err(Box::new(std::io::Error::other("db connection failed"))
                    as Box<dyn std::error::Error + Send + Sync>)
            })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let saved = rt.block_on(async { save_job_or_abort(make_job_model(), "test").await });

        assert!(saved.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn get_or_create_job_or_abort_returns_job_on_success() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
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
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let job = rt.block_on(async { get_or_create_job_or_abort(22, "test").await });

        let job = job.expect("get_or_create_job_or_abort should return Some on success");
        assert_eq!(job.id, 11);
        assert_eq!(job.job_id, Some(22));
        assert_eq!(job.scheduler_id, Some(33));
        assert_eq!(job.bundle_hash, "bundle-hash");
        assert_eq!(job.working_directory, "/tmp/workdir");
    }

    #[test]
    #[serial_test::serial]
    fn get_or_create_job_or_abort_returns_none_on_db_error() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            Box::pin(async move {
                Err(Box::new(std::io::Error::other("db connection failed"))
                    as Box<dyn std::error::Error + Send + Sync>)
            })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let job = rt.block_on(async { get_or_create_job_or_abort(22, "test").await });

        assert!(job.is_none());
    }

    #[test]
    fn run_bundle_bool_for_job_returns_true_on_success() {
        crate::tests::init_python_global();
        let fixture = crate::tests::fixtures::bundle_fixture::BundleFixture::new();
        let bundle_hash = "test_run_bundle_bool_for_job";
        fixture.write_job_cancel(bundle_hash, "True");
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_bundle_bool_for_job(
            "test_context",
            "cancel",
            bundle_hash,
            &json!({}),
        ));
        assert!(result);
    }

    #[test]
    fn run_bundle_bool_for_job_returns_false_on_spawn_blocking_failure() {
        BundleManager::reset_singleton_for_test();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_bundle_bool_for_job(
            "test_context",
            "cancel",
            "uninitialized_hash",
            &json!({}),
        ));
        assert!(!result);
    }

    #[test]
    fn archive_job_creates_archive_in_working_dir_on_success() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

        let job_model = job::Model {
            id: 1,
            job_id: Some(1234),
            scheduler_id: Some(4321),
            submitting: false,
            submitting_count: 0,
            bundle_hash: "hash".to_string(),
            working_directory: temp_dir.path().to_string_lossy().to_string(),
            running: false,
            deleted: false,
            deleting: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(archive_job(&job_model));

        assert!(result.is_ok());
        assert!(temp_dir.path().join(ARCHIVE_FILE_NAME).exists());
    }

    #[test]
    fn archive_job_returns_err_when_working_dir_missing() {
        let missing_dir = "/not/a/real/path/that/exists/archive_job_test";

        let job_model = job::Model {
            id: 1,
            job_id: Some(1234),
            scheduler_id: Some(4321),
            submitting: false,
            submitting_count: 0,
            bundle_hash: "hash".to_string(),
            working_directory: missing_dir.to_string(),
            running: false,
            deleted: false,
            deleting: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(archive_job(&job_model));

        assert!(result.is_err());
    }

    fn archive_entry_names(archive_path: &Path) -> Vec<String> {
        let file = std::fs::File::open(archive_path).unwrap();
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive
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
            .collect()
    }

    #[test]
    fn archive_dir_produces_valid_tar_gz_with_files_and_subdirs() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src = temp_dir.path().join("src");
        std::fs::create_dir_all(src.join("subdir")).unwrap();
        std::fs::write(src.join("root.txt"), b"root").unwrap();
        std::fs::write(src.join("subdir").join("nested.txt"), b"nested").unwrap();

        let archive_path = temp_dir.path().join("out.tar.gz");
        archive_dir(&src, &archive_path).unwrap();

        let names = archive_entry_names(&archive_path);
        assert!(names.iter().any(|n| n == "root.txt"));
        assert!(names.iter().any(|n| n == "subdir"));
        assert!(names.iter().any(|n| n == "subdir/nested.txt"));
    }

    #[test]
    fn archive_dir_excludes_pre_existing_archive_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let src = temp_dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("keep.txt"), b"keep").unwrap();
        std::fs::write(src.join(ARCHIVE_FILE_NAME), b"stale archive").unwrap();

        let archive_path = src.join(ARCHIVE_FILE_NAME);
        archive_dir(&src, &archive_path).unwrap();

        let names = archive_entry_names(&archive_path);
        assert!(names.iter().any(|n| n == "keep.txt"));
        assert!(!names.iter().any(|n| n == ARCHIVE_FILE_NAME));
    }

    #[test]
    fn archive_dir_returns_error_for_missing_source_dir() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let missing = temp_dir.path().join("does-not-exist");
        let archive_path = missing.join(ARCHIVE_FILE_NAME);

        assert!(archive_dir(&missing, &archive_path).is_err());
    }

    #[test]
    fn queue_job_update_queues_update_job_wire_format() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_queue_message()
            .with(always(), always(), eq(Priority::Medium))
            .times(1)
            .returning(|source, data, priority| {
                assert_eq!(source, "42");
                assert_eq!(priority, Priority::Medium);
                let mut msg = Message::from_data(data);
                assert_eq!(msg.id, UPDATE_JOB);
                assert_eq!(msg.source, "42");
                assert_eq!(msg.pop_uint(), 42);
                assert_eq!(msg.pop_string(), SYSTEM_SOURCE);
                assert_eq!(msg.pop_uint(), ERROR);
                assert_eq!(msg.pop_string(), "Job has failed");
            });
        set_websocket_client(Arc::new(mock));

        queue_job_update(42, SYSTEM_SOURCE, ERROR, "Job has failed");
    }

    #[test]
    #[serial_test::serial]
    fn check_job_status_returns_early_when_websocket_disconnected() {
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_is_connection_closed().return_const(true);
        mock.expect_is_server_ready().return_const(false);
        mock.expect_send_db_request().times(0);
        mock.expect_queue_message().times(0);
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(check_job_status(make_job_model(), true));
    }
}
