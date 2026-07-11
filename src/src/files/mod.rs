use crate::bundle_manager::BundleManager;
use crate::db;
use crate::messaging::{
    Message, Priority, FILE_CHUNK, FILE_DOWNLOAD_DETAILS, FILE_DOWNLOAD_ERROR, FILE_LIST,
    FILE_LIST_ERROR, FILE_UPLOAD_CHUNK, FILE_UPLOAD_COMPLETE, FILE_UPLOAD_ERROR,
    PAUSE_FILE_CHUNK_STREAM, RESUME_FILE_CHUNK_STREAM, SERVER_READY,
};
use crate::websocket::get_websocket_client;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::path::{Component, Path};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::header::{HeaderValue, AUTHORIZATION},
        protocol::Message as WsMessage,
    },
};
use tracing::{debug, error, trace, warn};

static FILE_LIST_SEMAPHORE: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(4)));

/// Caller note: this function spawns internally and does not need to be awaited.
pub fn handle_file_list(mut msg: Message) {
    let sem = FILE_LIST_SEMAPHORE.clone();
    tokio::spawn(async move {
        debug!("handle_file_list: received request");
        let _permit = sem.acquire_owned().await.expect("semaphore closed");
        let job_id = i64::from(msg.pop_uint());
        let uuid = msg.pop_string();
        let bundle_hash = msg.pop_string();
        let dir_path = msg.pop_string();
        let is_recursive = msg.pop_bool();
        debug!(
            "handle_file_list: job_id={}, uuid={}, bundle_hash={}, dir_path={}, recursive={}",
            job_id, uuid, bundle_hash, dir_path, is_recursive
        );

        let working_directory = if job_id != 0 {
            if let Ok(Some(job)) = db::get_job_by_job_id(job_id).await {
                if job.submitting {
                    send_file_list_error(&uuid, "Job is not submitted");
                    return;
                }
                job.working_directory
            } else {
                send_file_list_error(&uuid, "Job does not exist");
                return;
            }
        } else {
            let bundle_hash_clone = bundle_hash.clone();
            let dir_path_clone = dir_path.clone();
            tokio::task::spawn_blocking(move || {
                BundleManager::singleton().run_bundle_string(
                    "working_directory",
                    &bundle_hash_clone,
                    &json!(dir_path_clone),
                    "file_list",
                )
            })
            .await
            .map_err(|e| {
                error!("handle_file_list: Python FFI task panicked: {}", e);
                format!("Python FFI task failed: {e}")
            })
            .unwrap_or_else(|e| {
                error!("handle_file_list: spawn_blocking error: {}", e);
                String::new()
            })
        };

        let full_path = Path::new(&working_directory).join(&dir_path);
        let Ok(abs_path) = fs::canonicalize(&full_path).await else {
            send_file_list_error(&uuid, "Path to list files does not exist");
            return;
        };

        if !validate_path_is_within(&abs_path, &working_directory).await {
            send_file_list_error(&uuid, "Path to list files is outside the working directory");
            return;
        }

        if !fs::metadata(&abs_path).await.is_ok_and(|m| m.is_dir()) {
            send_file_list_error(&uuid, "Path to list files is not a directory");
            return;
        }

        let mut file_list = Vec::new();
        if is_recursive {
            let mut stack = vec![abs_path.clone()];
            while let Some(current_dir) = stack.pop() {
                if let Ok(mut entries) = fs::read_dir(current_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        let Ok(metadata) = entry.metadata().await else {
                            continue;
                        };
                        if metadata.is_symlink() {
                            continue;
                        }

                        let relative_path = path
                            .strip_prefix(&working_directory)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .into_owned();
                        file_list.push((relative_path, metadata.is_dir(), metadata.len()));

                        if metadata.is_dir() {
                            stack.push(path);
                        }
                    }
                }
            }
        } else if let Ok(mut entries) = fs::read_dir(&abs_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let Ok(metadata) = entry.metadata().await else {
                    continue;
                };
                if metadata.is_symlink() {
                    continue;
                }

                let relative_path = path
                    .strip_prefix(&working_directory)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                file_list.push((relative_path, metadata.is_dir(), metadata.len()));
            }
        }

        let mut result = Message::new(FILE_LIST, Priority::Highest, &uuid);
        result.push_string(&uuid);
        let file_count = u32::try_from(file_list.len()).unwrap_or(u32::MAX);
        result.push_uint(file_count);
        debug!(
            "handle_file_list: sending FILE_LIST response with {} files",
            file_count
        );
        for (path, is_dir, size) in file_list {
            result.push_string(&path);
            result.push_bool(is_dir);
            result.push_ulong(size);
        }
        let data_len = result.get_data().len();
        debug!(
            "handle_file_list: queuing FILE_LIST message ({} bytes)",
            data_len
        );
        get_websocket_client().queue_message(uuid, result.get_data().clone(), Priority::Highest);
        debug!("handle_file_list: FILE_LIST message queued");
    });
}

fn send_file_list_error(uuid: &str, error_msg: &str) {
    let mut result = Message::new(FILE_LIST_ERROR, Priority::Highest, uuid);
    result.push_string(uuid);
    result.push_string(error_msg);
    get_websocket_client().queue_message(
        uuid.to_string(),
        result.get_data().clone(),
        Priority::Highest,
    );
}

pub fn handle_file_download(mut msg: Message) {
    let job_id = i64::from(msg.pop_uint());
    let uuid = msg.pop_string();
    let bundle_hash = msg.pop_string();
    let mut file_path = msg.pop_string();

    tokio::spawn(async move {
        debug!(
            "handle_file_download: SPAWNED - job_id={}, uuid={}, bundle_hash={}, file_path={}",
            job_id, uuid, bundle_hash, file_path
        );
        debug!(
            "handle_file_download: STARTED - job_id={}, uuid={}, bundle_hash={}, file_path={}",
            job_id, uuid, bundle_hash, file_path
        );

        let config = crate::config::read_client_config();
        let ws_endpoint = config["websocketEndpoint"]
            .as_str()
            .unwrap_or("ws://127.0.0.1:8001/ws/");
        let request = match build_file_ws_request(ws_endpoint, &uuid) {
            Ok(request) => request,
            Err(e) => {
                warn!(
                    "handle_file_download: Failed to build file download request: {}",
                    e
                );
                return;
            }
        };

        let (ws_stream, _) = match connect_async(request).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "handle_file_download: Failed to connect for file download: {}",
                    e
                );
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        if wait_for_server_ready(&mut ws_receiver).await.is_none() {
            warn!("handle_file_download: Failed to receive SERVER_READY");
            return;
        }

        debug!("handle_file_download: SERVER_READY received, resolving working directory for job_id={}", job_id);
        let working_directory = if job_id != 0 {
            debug!(
                "handle_file_download: Looking up job {} in database",
                job_id
            );
            match db::get_job_by_job_id(job_id).await {
                Ok(Some(job)) => {
                    debug!(
                        "handle_file_download: Job found, submitting={}",
                        job.submitting
                    );
                    if job.submitting {
                        warn!("handle_file_download: Job is not submitted, sending error");
                        send_download_error(&mut ws_sender, &uuid, "Job is not submitted").await;
                        return;
                    }
                    job.working_directory
                }
                Ok(None) => {
                    warn!("handle_file_download: Job {} not found in database", job_id);
                    send_download_error(&mut ws_sender, &uuid, "Job does not exist").await;
                    return;
                }
                Err(e) => {
                    warn!(
                        "handle_file_download: Database error for job {}: {}",
                        job_id, e
                    );
                    send_download_error(&mut ws_sender, &uuid, &format!("Database error: {e}"))
                        .await;
                    return;
                }
            }
        } else {
            debug!("handle_file_download: Using bundle manager for working_directory");
            let bundle_hash_clone = bundle_hash.clone();
            let file_path_clone = file_path.clone();
            tokio::task::spawn_blocking(move || {
                BundleManager::singleton().run_bundle_string(
                    "working_directory",
                    &bundle_hash_clone,
                    &json!(file_path_clone),
                    "file_download",
                )
            })
            .await
            .map_err(|e| {
                error!("handle_file_download: Python FFI task panicked: {}", e);
                format!("Python FFI task failed: {e}")
            })
            .unwrap_or_else(|e| {
                error!("handle_file_download: spawn_blocking error: {}", e);
                String::new()
            })
        };

        debug!(
            "handle_file_download: working_directory={}, file_path={}",
            working_directory, file_path
        );
        file_path = file_path.trim_start_matches('/').to_string();

        let full_path = Path::new(&working_directory).join(&file_path);
        debug!("handle_file_download: full_path={:?}", full_path);
        let abs_path = match fs::canonicalize(&full_path).await {
            Ok(path) => {
                debug!("handle_file_download: canonicalized path={:?}", path);
                path
            }
            Err(e) => {
                warn!("handle_file_download: Failed to canonicalize path: {}", e);
                send_download_error(
                    &mut ws_sender,
                    &uuid,
                    "Path to file download does not exist",
                )
                .await;
                return;
            }
        };

        if !validate_path_is_within(&abs_path, &working_directory).await {
            warn!("handle_file_download: Path validation failed - outside working directory");
            send_download_error(
                &mut ws_sender,
                &uuid,
                "Path to file download is outside the working directory",
            )
            .await;
            return;
        }

        debug!("handle_file_download: Getting file metadata");
        let file_meta = match fs::metadata(&abs_path).await {
            Ok(m) if m.is_file() => {
                debug!("handle_file_download: File found, size={} bytes", m.len());
                m
            }
            Ok(m) => {
                warn!(
                    "handle_file_download: Path is not a file (is_directory={})",
                    m.is_dir()
                );
                send_download_error(&mut ws_sender, &uuid, "Path to file download is not a file")
                    .await;
                return;
            }
            Err(e) => {
                warn!("handle_file_download: Failed to get file metadata: {}", e);
                send_download_error(
                    &mut ws_sender,
                    &uuid,
                    &format!("Failed to get file metadata: {e}"),
                )
                .await;
                return;
            }
        };

        let file_size = file_meta.len();
        debug!(
            "handle_file_download: Sending FILE_DOWNLOAD_DETAILS (file_size={} bytes)",
            file_size
        );
        let mut details_msg = Message::new(FILE_DOWNLOAD_DETAILS, Priority::Highest, &uuid);
        details_msg.push_ulong(file_size);
        if let Err(e) = ws_sender
            .send(WsMessage::Binary(details_msg.get_data().clone().into()))
            .await
        {
            warn!(
                "handle_file_download: Failed to send FILE_DOWNLOAD_DETAILS: {}",
                e
            );
            return;
        }
        debug!("handle_file_download: FILE_DOWNLOAD_DETAILS sent successfully");

        debug!("handle_file_download: Opening file for reading");
        let mut file = match File::open(&abs_path).await {
            Ok(f) => {
                debug!("handle_file_download: File opened successfully");
                f
            }
            Err(e) => {
                warn!("Failed to open file for download: {}", e);
                send_download_error(&mut ws_sender, &uuid, "Failed to open file for download")
                    .await;
                return;
            }
        };
        let mut buffer = vec![0u8; 64 * 1024];
        let download_start = std::time::Instant::now();
        let mut total_bytes = 0;

        let paused = Arc::new(Notify::new());
        let is_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let is_paused_clone = is_paused.clone();
        let paused_clone = paused.clone();
        tokio::spawn(async move {
            while let Some(Ok(ws_msg)) = ws_receiver.next().await {
                if let WsMessage::Binary(data) = ws_msg {
                    let m = Message::from_data(data.to_vec());
                    if m.id == PAUSE_FILE_CHUNK_STREAM {
                        is_paused_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    } else if m.id == RESUME_FILE_CHUNK_STREAM {
                        is_paused_clone.store(false, std::sync::atomic::Ordering::SeqCst);
                        paused_clone.notify_one();
                    }
                }
            }
        });

        loop {
            if is_paused.load(std::sync::atomic::Ordering::SeqCst) {
                paused.notified().await;
            }

            let n = match file.read(&mut buffer).await {
                Ok(0) => {
                    trace!("handle_file_download: End of file reached");
                    break;
                }
                Ok(n) => {
                    trace!("handle_file_download: Read {} bytes from file", n);
                    n
                }
                Err(e) => {
                    warn!("Error reading file: {}", e);
                    let mut err_msg = Message::new(FILE_DOWNLOAD_ERROR, Priority::Highest, &uuid);
                    err_msg.push_string("Exception reading file");
                    let _ = ws_sender
                        .send(WsMessage::Binary(err_msg.get_data().clone().into()))
                        .await;
                    return;
                }
            };

            total_bytes += n;
            trace!(
                "handle_file_download: Sending chunk {} (total: {} bytes)",
                total_bytes,
                n
            );

            let mut chunk_msg = Message::new(FILE_CHUNK, Priority::Highest, &uuid);
            chunk_msg.push_bytes(&buffer[..n]);
            match ws_sender
                .send(WsMessage::Binary(chunk_msg.get_data().clone().into()))
                .await
            {
                Ok(()) => trace!("handle_file_download: Chunk sent successfully"),
                Err(e) => {
                    warn!("handle_file_download: Failed to send chunk: {}", e);
                    break;
                }
            }
            // Yield to allow pause/resume messages to be processed
            tokio::task::yield_now().await;
        }
        debug!(
            "handle_file_download: COMPLETED - downloaded {} bytes in {:?}",
            total_bytes,
            download_start.elapsed()
        );
    });
}

async fn send_download_error(
    ws_sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    uuid: &str,
    error_msg: &str,
) {
    let mut result = Message::new(FILE_DOWNLOAD_ERROR, Priority::Highest, uuid);
    result.push_string(error_msg);
    let _ = ws_sender
        .send(WsMessage::Binary(result.get_data().clone().into()))
        .await;
}

pub fn handle_file_upload(mut msg: Message) {
    let uuid = msg.pop_string();
    let job_id = i64::from(msg.pop_uint());
    let bundle_hash = msg.pop_string();
    let target_path = msg.pop_string();
    let file_size = msg.pop_ulong();

    // Read config BEFORE spawning to capture the correct URL for this upload
    let config = crate::config::read_client_config();
    let ws_endpoint = config["websocketEndpoint"]
        .as_str()
        .unwrap_or("ws://127.0.0.1:8001/ws/");

    handle_file_upload_internal(
        uuid,
        job_id,
        bundle_hash,
        target_path,
        file_size,
        ws_endpoint.to_string(),
    );
}

pub fn handle_file_upload_with_url(mut msg: Message, ws_endpoint: String) {
    let uuid = msg.pop_string();
    let job_id = i64::from(msg.pop_uint());
    let bundle_hash = msg.pop_string();
    let target_path = msg.pop_string();
    let file_size = msg.pop_ulong();

    handle_file_upload_internal(
        uuid,
        job_id,
        bundle_hash,
        target_path,
        file_size,
        ws_endpoint,
    );
}

fn handle_file_upload_internal(
    uuid: String,
    job_id: i64,
    bundle_hash: String,
    mut target_path: String,
    file_size: u64,
    ws_endpoint: String,
) {
    tokio::spawn(async move {
        let request = match build_file_ws_request(&ws_endpoint, &uuid) {
            Ok(request) => request,
            Err(e) => {
                warn!("Failed to build file upload request: {}", e);
                return;
            }
        };

        let (ws_stream, _) = match connect_async(request).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to connect for file upload: {}", e);
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        if wait_for_server_ready(&mut ws_receiver).await.is_none() {
            return;
        }

        let ready_msg = Message::new(SERVER_READY, Priority::Highest, &uuid);
        let _ = ws_sender
            .send(WsMessage::Binary(ready_msg.get_data().clone().into()))
            .await;

        let working_directory = if job_id != 0 {
            if let Ok(Some(job)) = db::get_job_by_job_id(job_id).await {
                if job.submitting {
                    send_upload_error(&mut ws_sender, &uuid, "Job is not submitted").await;
                    return;
                }
                job.working_directory
            } else {
                send_upload_error(&mut ws_sender, &uuid, "Job does not exist").await;
                return;
            }
        } else {
            let bundle_hash_clone = bundle_hash.clone();
            let target_path_clone = target_path.clone();
            let working_dir_result = tokio::task::spawn_blocking(move || {
                BundleManager::singleton().run_bundle_string(
                    "working_directory",
                    &bundle_hash_clone,
                    &json!(target_path_clone),
                    "file_upload",
                )
            })
            .await
            .map_err(|e| {
                error!(
                    "handle_file_upload_internal: Python FFI task panicked: {}",
                    e
                );
                format!("Python FFI task failed: {e}")
            })
            .unwrap_or_else(|e| {
                error!("handle_file_upload_internal: spawn_blocking error: {}", e);
                String::new()
            });
            working_dir_result
        };

        target_path = target_path.trim_start_matches('/').to_string();

        let full_path = Path::new(&working_directory).join(&target_path);

        if !validate_path_is_within(&full_path, &working_directory).await {
            send_upload_error(
                &mut ws_sender,
                &uuid,
                "Target path for file upload is outside the working directory",
            )
            .await;
            return;
        }

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            let _ = fs::create_dir_all(parent).await;
        }

        let mut file = match File::create(&full_path).await {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to create file: {}", e);
                send_upload_error(
                    &mut ws_sender,
                    &uuid,
                    "Failed to open target file for writing",
                )
                .await;
                return;
            }
        };

        let mut received_size = 0u64;
        let upload_start = std::time::Instant::now();
        while let Some(Ok(ws_msg)) = ws_receiver.next().await {
            if let WsMessage::Binary(data) = ws_msg {
                let mut m = Message::from_data(data.to_vec());
                if m.id == FILE_UPLOAD_CHUNK {
                    let chunk = m.pop_bytes();
                    if let Err(e) = file.write_all(&chunk).await {
                        warn!("Failed to write chunk: {}", e);
                        let _ = fs::remove_file(&full_path).await;
                        send_upload_error(&mut ws_sender, &uuid, "Failed to write chunk to file")
                            .await;
                        return;
                    }
                    received_size += chunk.len() as u64;
                } else if m.id == FILE_UPLOAD_COMPLETE {
                    if received_size != file_size {
                        let _ = fs::remove_file(&full_path).await;
                        send_upload_error(
                            &mut ws_sender,
                            &uuid,
                            &format!(
                                "File size mismatch: expected {file_size}, got {received_size}"
                            ),
                        )
                        .await;
                        return;
                    }
                    if let Err(e) = file.flush().await {
                        warn!("Failed to flush uploaded file: {}", e);
                        let _ = fs::remove_file(&full_path).await;
                        send_upload_error(
                            &mut ws_sender,
                            &uuid,
                            "Failed to finalize uploaded file",
                        )
                        .await;
                        return;
                    }
                    if let Err(e) = file.sync_all().await {
                        warn!("Failed to sync uploaded file: {}", e);
                        let _ = fs::remove_file(&full_path).await;
                        send_upload_error(
                            &mut ws_sender,
                            &uuid,
                            "Failed to finalize uploaded file",
                        )
                        .await;
                        return;
                    }
                    drop(file);
                    let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &uuid);
                    let _ = ws_sender
                        .send(WsMessage::Binary(complete_msg.get_data().clone().into()))
                        .await;
                    debug!(
                        "handle_file_upload: uploaded {} bytes in {:?}",
                        received_size,
                        upload_start.elapsed()
                    );
                    return;
                }
            }
        }
    });
}

fn build_file_ws_request(
    ws_endpoint: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    let mut request = ws_endpoint
        .into_client_request()
        .map_err(|e| format!("invalid websocket endpoint: {e}"))?;
    let auth_value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| format!("invalid authorization header: {e}"))?;
    request.headers_mut().insert(AUTHORIZATION, auth_value);
    Ok(request)
}

/// Wait for a `SERVER_READY` message with a 10-second timeout.
async fn wait_for_server_ready(
    ws_receiver: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Option<Message> {
    let handshake = tokio::time::timeout(Duration::from_secs(10), ws_receiver.next()).await;
    match handshake {
        Ok(Some(Ok(WsMessage::Binary(data)))) => {
            let msg = Message::from_data(data.to_vec());
            if msg.id != SERVER_READY {
                warn!("Expected SERVER_READY, got {}", msg.id);
                return None;
            }
            Some(msg)
        }
        Ok(Some(Ok(_))) => {
            warn!("Expected binary SERVER_READY, got unexpected frame");
            None
        }
        Ok(Some(Err(e))) => {
            warn!("Handshake error: {}", e);
            None
        }
        Ok(None) => {
            warn!("Server closed connection before sending SERVER_READY");
            None
        }
        Err(_) => {
            warn!("Timeout waiting for SERVER_READY");
            None
        }
    }
}

async fn validate_path_is_within(target_path: &Path, working_directory: &str) -> bool {
    let Ok(canonical_working) = fs::canonicalize(working_directory).await else {
        return false;
    };

    // Canonicalize the full target path (if it exists)
    if let Ok(canonical_target) = fs::canonicalize(target_path).await {
        return canonical_target.starts_with(&canonical_working);
    }

    // For non-existent paths (e.g., uploads), build the longest existing prefix
    // by checking parent directories, then verify no path components escape.
    let mut current = target_path.to_path_buf();
    // Walk up until we find a path that exists
    loop {
        if current.exists() {
            break;
        }
        if !current.pop() {
            return false;
        }
    }

    let Ok(canonical_prefix) = fs::canonicalize(&current).await else {
        return false;
    };

    if !canonical_prefix.starts_with(&canonical_working) {
        return false;
    }

    // Verify remaining components don't contain parent dir references
    let remaining = target_path.strip_prefix(&current).unwrap_or(target_path);
    for component in remaining.components() {
        if matches!(component, Component::ParentDir) {
            return false;
        }
    }

    true
}

async fn send_upload_error(
    ws_sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    uuid: &str,
    error_msg: &str,
) {
    let mut result = Message::new(FILE_UPLOAD_ERROR, Priority::Highest, uuid);
    result.push_string(error_msg);
    let _ = ws_sender
        .send(WsMessage::Binary(result.get_data().clone().into()))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

    fn run_validate(target: &Path, working_dir: &str) -> bool {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(validate_path_is_within(target, working_dir))
    }

    #[test]
    fn test_path_within_working_dir() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let sub = tmp.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        let file = sub.join("test.txt");
        fs::write(&file, "data").unwrap();

        assert!(run_validate(&file, &wd), "file inside wd should pass");
        assert!(run_validate(&sub, &wd), "subdir inside wd should pass");
        assert!(run_validate(tmp.path(), &wd), "wd itself should pass");
    }

    #[test]
    fn test_path_outside_working_dir() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let file = outside.path().join("secret.txt");
        fs::write(&file, "data").unwrap();

        assert!(!run_validate(&file, &wd), "file outside wd should fail");
    }

    #[test]
    fn test_path_traversal_escape() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let escape = tmp.path().join("..").join("..").join("etc").join("passwd");
        assert!(!run_validate(&escape, &wd), "path traversal should fail");

        // Double-dot that resolves within wd is valid (canonicalization collapses it)
        let sub = tmp.path().join("subdir").join("..").join("subdir");
        fs::create_dir(tmp.path().join("subdir")).unwrap();
        assert!(
            run_validate(&sub, &wd),
            ".. that resolves inside wd should pass"
        );

        // Double-dot that resolves outside wd should fail
        let outside_escape = tmp.path().join("..").join("..").join("tmp");
        assert!(
            !run_validate(&outside_escape, &wd),
            ".. that resolves outside wd should fail"
        );
    }

    #[test]
    fn test_non_existent_path_for_upload() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let new_file = tmp.path().join("new_dir").join("new_file.txt");
        // Neither new_dir nor new_file exists yet — upload case
        assert!(
            run_validate(&new_file, &wd),
            "non-existent path within wd should pass"
        );

        let escape = tmp.path().join("..").join("outside_file.txt");
        assert!(
            !run_validate(&escape, &wd),
            "non-existent path escaping wd should fail"
        );
    }

    #[test]
    fn test_symlink_to_outside_rejected() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "data").unwrap();
        let link = tmp.path().join("link_to_outside");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        assert!(!run_validate(&link, &wd), "symlink to outside should fail");
    }

    #[test]
    fn test_symlink_within_working_dir_allowed() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let target = tmp.path().join("data.txt");
        fs::write(&target, "data").unwrap();
        let link = tmp.path().join("link_to_data");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(
            run_validate(&link, &wd),
            "symlink to a file inside wd should pass"
        );
    }

    #[test]
    fn test_invalid_working_directory() {
        let tmp = TempDir::new().unwrap();
        let wd = "/nonexistent/path/that/does/not/exist";
        let file = tmp.path().join("test.txt");
        fs::write(&file, "data").unwrap();

        assert!(!run_validate(&file, wd), "invalid wd should fail");
    }

    #[test]
    fn test_non_existent_path_with_all_parents_outside() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let escape = Path::new("/").join("etc").join("nonexistent_file");
        assert!(
            !run_validate(&escape, &wd),
            "path at root outside wd should fail"
        );
    }

    #[test]
    fn test_non_existent_path_rejects_parent_dir_in_suffix() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        let sub = tmp.path().join("uploads");
        fs::create_dir(&sub).unwrap();

        // Prefix exists inside wd, but remaining suffix climbs out via "..".
        let escape = sub.join("..").join("..").join("etc").join("passwd");
        assert!(
            !run_validate(&escape, &wd),
            "non-existent path with .. in suffix should fail"
        );

        let nested_escape = sub.join("nested").join("..").join("..").join("outside.txt");
        assert!(
            !run_validate(&nested_escape, &wd),
            "nested non-existent path with .. in suffix should fail"
        );
    }

    #[test]
    fn test_build_file_ws_request_sets_bearer_header() {
        let request = build_file_ws_request("ws://127.0.0.1:9001/ws/", "test-token").unwrap();

        assert_eq!(request.uri().to_string(), "ws://127.0.0.1:9001/ws/");
        assert_eq!(
            request.headers().get(AUTHORIZATION).unwrap(),
            "Bearer test-token"
        );
    }

    #[test]
    fn test_build_file_ws_request_accepts_wss_endpoint() {
        let request = build_file_ws_request("wss://example.com:443/ws/", "secure-token").unwrap();

        assert_eq!(request.uri().to_string(), "wss://example.com:443/ws/");
        assert_eq!(
            request.headers().get(AUTHORIZATION).unwrap(),
            "Bearer secure-token"
        );
    }

    #[test]
    fn test_build_file_ws_request_rejects_invalid_endpoint() {
        let err = build_file_ws_request("not a websocket url", "test-token").unwrap_err();
        assert!(err.contains("invalid websocket endpoint"));
    }

    #[test]
    fn test_build_file_ws_request_rejects_invalid_token() {
        let err =
            build_file_ws_request("ws://127.0.0.1:9001/ws/", "token\nwith-newline").unwrap_err();
        assert!(err.contains("invalid authorization header"));
    }
}
