use crate::bundle_manager::resolve_working_directory;
use crate::db;
use crate::messaging::{
    Message, Priority, FILE_CHUNK, FILE_DOWNLOAD_DETAILS, FILE_DOWNLOAD_ERROR, FILE_LIST,
    FILE_LIST_ERROR, FILE_UPLOAD_CHUNK, FILE_UPLOAD_COMPLETE, FILE_UPLOAD_ERROR,
    PAUSE_FILE_CHUNK_STREAM, RESUME_FILE_CHUNK_STREAM, SERVER_READY,
};
use crate::websocket::get_websocket_client;
use futures_util::{Sink, SinkExt, StreamExt};
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

const FILE_LIST_CONCURRENCY_LIMIT: usize = 4;
const DOWNLOAD_CHUNK_SIZE: usize = 64 * 1024;
const SERVER_READY_TIMEOUT_SECS: u64 = 10;
const FILE_WS_CONNECT_TIMEOUT_SECS: u64 = 10;

static FILE_LIST_SEMAPHORE: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(FILE_LIST_CONCURRENCY_LIMIT)));

#[cfg(test)]
pub(crate) fn close_file_list_semaphore_for_test() {
    FILE_LIST_SEMAPHORE.close();
}

/// Client-facing error from a working-directory lookup.
enum JobLookupError {
    NotSubmitted,
    NotFound,
    Database(String),
}

impl JobLookupError {
    /// The exact error string sent to the client over the wire.
    fn client_message(&self) -> String {
        match self {
            Self::NotSubmitted => "Job is not submitted".to_string(),
            Self::NotFound => "Job does not exist".to_string(),
            Self::Database(e) => format!("Database error: {e}"),
        }
    }
}

/// Looks up a job's working directory in the database, returning the exact
/// client-facing error message when the lookup fails.
async fn lookup_job_working_directory(job_id: i64) -> Result<String, JobLookupError> {
    match db::get_job_by_job_id(job_id).await {
        Ok(Some(job)) => {
            if job.submitting {
                Err(JobLookupError::NotSubmitted)
            } else {
                Ok(job.working_directory)
            }
        }
        Ok(None) => Err(JobLookupError::NotFound),
        Err(e) => Err(JobLookupError::Database(e)),
    }
}

/// Caller note: this function spawns internally and does not need to be awaited.
pub fn handle_file_list(mut msg: Message) {
    let sem = FILE_LIST_SEMAPHORE.clone();
    tokio::spawn(async move {
        debug!("handle_file_list: received request");
        let _permit = match sem.acquire_owned().await {
            Ok(permit) => permit,
            Err(e) => {
                error!(
                    "handle_file_list: semaphore closed, dropping request: {}",
                    e
                );
                return;
            }
        };
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
            match lookup_job_working_directory(job_id).await {
                Ok(working_directory) => working_directory,
                Err(err) => {
                    if let JobLookupError::Database(e) = &err {
                        error!("handle_file_list: Database error for job {}: {}", job_id, e);
                    }
                    send_file_list_error(&uuid, &err.client_message());
                    return;
                }
            }
        } else {
            resolve_working_directory(
                &bundle_hash,
                json!(dir_path.clone()),
                "file_list",
                "handle_file_list",
            )
            .await
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
                        if let Some((relative_path, is_dir, size)) =
                            collect_dir_entry(entry, &working_directory).await
                        {
                            file_list.push((relative_path, is_dir, size));
                            if is_dir {
                                stack.push(path);
                            }
                        }
                    }
                }
            }
        } else if let Ok(mut entries) = fs::read_dir(&abs_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some(entry_info) = collect_dir_entry(entry, &working_directory).await {
                    file_list.push(entry_info);
                }
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

async fn collect_dir_entry(
    entry: fs::DirEntry,
    working_directory: &str,
) -> Option<(String, bool, u64)> {
    let path = entry.path();
    let metadata = match entry.metadata().await {
        Ok(metadata) => metadata,
        Err(e) => {
            warn!(
                "collect_dir_entry: failed to read metadata for {:?}: {}",
                path, e
            );
            return None;
        }
    };
    if metadata.is_symlink() {
        return None;
    }

    let relative_path = path
        .strip_prefix(working_directory)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned();
    Some((relative_path, metadata.is_dir(), metadata.len()))
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

fn get_ws_endpoint_from_config() -> String {
    let config = crate::config::read_client_config();
    crate::config::ensure_websocket_endpoint_trailing_slash(
        config["websocketEndpoint"]
            .as_str()
            .unwrap_or("ws://127.0.0.1:8001/ws/"),
    )
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

        let ws_endpoint = get_ws_endpoint_from_config();
        let request = match build_file_ws_request(&ws_endpoint, &uuid) {
            Ok(request) => request,
            Err(e) => {
                warn!(
                    "handle_file_download: Failed to build file download request: {}",
                    e
                );
                return;
            }
        };

        let (ws_stream, _) = match tokio::time::timeout(
            Duration::from_secs(FILE_WS_CONNECT_TIMEOUT_SECS),
            connect_async(request),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!(
                    "handle_file_download: Failed to connect for file download: {}",
                    e
                );
                return;
            }
            Err(_) => {
                warn!("handle_file_download: Timed out connecting for file download");
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
            match lookup_job_working_directory(job_id).await {
                Ok(working_directory) => {
                    debug!("handle_file_download: Job found, submitting=false");
                    working_directory
                }
                Err(err) => {
                    match &err {
                        JobLookupError::NotSubmitted => {
                            warn!("handle_file_download: Job is not submitted, sending error");
                        }
                        JobLookupError::NotFound => {
                            warn!("handle_file_download: Job {} not found in database", job_id);
                        }
                        JobLookupError::Database(e) => {
                            warn!(
                                "handle_file_download: Database error for job {}: {}",
                                job_id, e
                            );
                        }
                    }
                    send_file_error(
                        &mut ws_sender,
                        &uuid,
                        &err.client_message(),
                        FILE_DOWNLOAD_ERROR,
                    )
                    .await;
                    return;
                }
            }
        } else {
            debug!("handle_file_download: Using bundle manager for working_directory");
            resolve_working_directory(
                &bundle_hash,
                json!(file_path.clone()),
                "file_download",
                "handle_file_download",
            )
            .await
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
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    "Path to file download does not exist",
                    FILE_DOWNLOAD_ERROR,
                )
                .await;
                return;
            }
        };

        if !validate_path_is_within(&abs_path, &working_directory).await {
            warn!("handle_file_download: Path validation failed - outside working directory");
            send_file_error(
                &mut ws_sender,
                &uuid,
                "Path to file download is outside the working directory",
                FILE_DOWNLOAD_ERROR,
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
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    "Path to file download is not a file",
                    FILE_DOWNLOAD_ERROR,
                )
                .await;
                return;
            }
            Err(e) => {
                warn!("handle_file_download: Failed to get file metadata: {}", e);
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    &format!("Failed to get file metadata: {e}"),
                    FILE_DOWNLOAD_ERROR,
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
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    "Failed to open file for download",
                    FILE_DOWNLOAD_ERROR,
                )
                .await;
                return;
            }
        };
        let mut buffer = vec![0u8; DOWNLOAD_CHUNK_SIZE];
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
            // Connection closed: if a PAUSE was received without a matching
            // RESUME, wake the download loop so it observes the dead connection
            // and exits instead of blocking forever on the pause notification.
            is_paused_clone.store(false, std::sync::atomic::Ordering::SeqCst);
            paused_clone.notify_one();
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
                    send_file_error(
                        &mut ws_sender,
                        &uuid,
                        "Exception reading file",
                        FILE_DOWNLOAD_ERROR,
                    )
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
        if total_bytes as u64 != file_size {
            warn!(
                "handle_file_download: File size mismatch: expected {file_size} bytes, got {total_bytes} bytes (file truncated or modified during download)"
            );
        }
        debug!(
            "handle_file_download: COMPLETED - downloaded {} bytes in {:?}",
            total_bytes,
            download_start.elapsed()
        );
    });
}

async fn send_file_error<S, E>(ws_sender: &mut S, uuid: &str, error_msg: &str, msg_id: u32)
where
    S: Sink<WsMessage, Error = E> + Unpin,
    E: std::fmt::Display,
{
    let mut result = Message::new(msg_id, Priority::Highest, uuid);
    result.push_string(error_msg);
    if let Err(e) = ws_sender
        .send(WsMessage::Binary(result.get_data().clone().into()))
        .await
    {
        warn!(
            "send_file_error: Failed to send error message (msg_id={}): {}",
            msg_id, e
        );
    }
}

struct UploadFields {
    uuid: String,
    job_id: i64,
    bundle_hash: String,
    target_path: String,
    file_size: u64,
}

fn parse_upload_fields(msg: &mut Message) -> UploadFields {
    UploadFields {
        uuid: msg.pop_string(),
        job_id: i64::from(msg.pop_uint()),
        bundle_hash: msg.pop_string(),
        target_path: msg.pop_string(),
        file_size: msg.pop_ulong(),
    }
}

pub fn handle_file_upload(mut msg: Message) {
    let fields = parse_upload_fields(&mut msg);

    // Read config BEFORE spawning to capture the correct URL for this upload
    let ws_endpoint = get_ws_endpoint_from_config();

    handle_file_upload_internal(
        fields.uuid,
        fields.job_id,
        fields.bundle_hash,
        fields.target_path,
        fields.file_size,
        ws_endpoint,
    );
}

pub fn handle_file_upload_with_url(mut msg: Message, ws_endpoint: String) {
    let fields = parse_upload_fields(&mut msg);

    handle_file_upload_internal(
        fields.uuid,
        fields.job_id,
        fields.bundle_hash,
        fields.target_path,
        fields.file_size,
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

        let (ws_stream, _) = match tokio::time::timeout(
            Duration::from_secs(FILE_WS_CONNECT_TIMEOUT_SECS),
            connect_async(request),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("Failed to connect for file upload: {}", e);
                return;
            }
            Err(_) => {
                warn!("Timed out connecting for file upload");
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        if wait_for_server_ready(&mut ws_receiver).await.is_none() {
            return;
        }

        let ready_msg = Message::new(SERVER_READY, Priority::Highest, &uuid);
        if let Err(e) = ws_sender
            .send(WsMessage::Binary(ready_msg.get_data().clone().into()))
            .await
        {
            warn!(
                "handle_file_upload_internal: Failed to send SERVER_READY ack: {}",
                e
            );
            return;
        }

        let working_directory = if job_id != 0 {
            match lookup_job_working_directory(job_id).await {
                Ok(working_directory) => working_directory,
                Err(err) => {
                    if let JobLookupError::Database(e) = &err {
                        warn!(
                            "handle_file_upload_internal: Database error for job {}: {}",
                            job_id, e
                        );
                    }
                    send_file_error(
                        &mut ws_sender,
                        &uuid,
                        &err.client_message(),
                        FILE_UPLOAD_ERROR,
                    )
                    .await;
                    return;
                }
            }
        } else {
            resolve_working_directory(
                &bundle_hash,
                json!(target_path.clone()),
                "file_upload",
                "handle_file_upload_internal",
            )
            .await
        };

        target_path = target_path.trim_start_matches('/').to_string();

        let full_path = Path::new(&working_directory).join(&target_path);

        if !validate_path_is_within(&full_path, &working_directory).await {
            send_file_error(
                &mut ws_sender,
                &uuid,
                "Target path for file upload is outside the working directory",
                FILE_UPLOAD_ERROR,
            )
            .await;
            return;
        }

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            if let Err(e) = fs::create_dir_all(parent).await {
                warn!(
                    "handle_file_upload_internal: Failed to create parent directory {:?}: {}",
                    parent, e
                );
            }
        }

        let mut file = match File::create(&full_path).await {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to create file: {}", e);
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    "Failed to open target file for writing",
                    FILE_UPLOAD_ERROR,
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
                        send_file_error(
                            &mut ws_sender,
                            &uuid,
                            "Failed to write chunk to file",
                            FILE_UPLOAD_ERROR,
                        )
                        .await;
                        return;
                    }
                    received_size += chunk.len() as u64;
                } else if m.id == FILE_UPLOAD_COMPLETE {
                    if received_size != file_size {
                        let _ = fs::remove_file(&full_path).await;
                        send_file_error(
                            &mut ws_sender,
                            &uuid,
                            &format!(
                                "File size mismatch: expected {file_size}, got {received_size}"
                            ),
                            FILE_UPLOAD_ERROR,
                        )
                        .await;
                        return;
                    }
                    if !finalize_upload_file(&mut file, &full_path, &mut ws_sender, &uuid).await {
                        return;
                    }
                    drop(file);
                    let complete_msg = Message::new(FILE_UPLOAD_COMPLETE, Priority::Highest, &uuid);
                    if let Err(e) = ws_sender
                        .send(WsMessage::Binary(complete_msg.get_data().clone().into()))
                        .await
                    {
                        warn!(
                            "handle_file_upload: Failed to send FILE_UPLOAD_COMPLETE: {}",
                            e
                        );
                    }
                    debug!(
                        "handle_file_upload: uploaded {} bytes in {:?}",
                        received_size,
                        upload_start.elapsed()
                    );
                    return;
                }
            }
        }

        // Connection dropped before FILE_UPLOAD_COMPLETE — remove the partial
        // file so it isn't mistaken for a complete upload.
        warn!("handle_file_upload: connection dropped before FILE_UPLOAD_COMPLETE, removing partial file");
        let _ = fs::remove_file(&full_path).await;
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
    let handshake = tokio::time::timeout(
        Duration::from_secs(SERVER_READY_TIMEOUT_SECS),
        ws_receiver.next(),
    )
    .await;
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
            warn!("Timeout waiting for SERVER_READY after {SERVER_READY_TIMEOUT_SECS}s");
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

/// Async file-finalization surface required by `finalize_upload_file`.
/// Abstracted so unit tests can inject flush/sync failures.
trait FinalizeFile {
    async fn flush(&mut self) -> std::io::Result<()>;
    async fn sync_all(&mut self) -> std::io::Result<()>;
}

impl FinalizeFile for File {
    async fn flush(&mut self) -> std::io::Result<()> {
        AsyncWriteExt::flush(self).await
    }

    async fn sync_all(&mut self) -> std::io::Result<()> {
        File::sync_all(self).await
    }
}

/// Flush and sync the uploaded file to disk. On failure, removes the partial
/// file and sends the upload error, returning `false`.
async fn finalize_upload_file<F, S, E>(
    file: &mut F,
    full_path: &Path,
    ws_sender: &mut S,
    uuid: &str,
) -> bool
where
    F: FinalizeFile,
    S: Sink<WsMessage, Error = E> + Unpin,
    E: std::fmt::Display,
{
    if let Err(e) = file.flush().await {
        warn!("Failed to flush uploaded file: {}", e);
        let _ = fs::remove_file(full_path).await;
        send_file_error(
            ws_sender,
            uuid,
            "Failed to finalize uploaded file",
            FILE_UPLOAD_ERROR,
        )
        .await;
        return false;
    }
    if let Err(e) = file.sync_all().await {
        warn!("Failed to sync uploaded file: {}", e);
        let _ = fs::remove_file(full_path).await;
        send_file_error(
            ws_sender,
            uuid,
            "Failed to finalize uploaded file",
            FILE_UPLOAD_ERROR,
        )
        .await;
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::{DB_RESPONSE, UPLOAD_FILE};
    use crate::tests::fixtures::websocket_server_fixture::WebsocketServerFixture;
    use crate::websocket::{
        reset_websocket_client_for_test, set_websocket_client, MockWebsocketClient,
    };
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn make_job_lookup_response(submitting: bool, working_directory: &str) -> Message {
        let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
        resp.push_uint(1);
        resp.push_ulong(11);
        resp.push_ulong(22);
        resp.push_ulong(33);
        resp.push_bool(submitting);
        resp.push_uint(4);
        resp.push_string("bundle-hash");
        resp.push_string(working_directory);
        resp.push_bool(true);
        resp.push_bool(false);
        resp.push_bool(false);
        resp
    }

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

    #[test]
    fn parse_upload_fields_reads_wire_order() {
        let mut msg = Message::new(UPLOAD_FILE, Priority::Highest, "client");
        msg.push_string("uuid-123");
        msg.push_uint(42);
        msg.push_string("bundle-hash");
        msg.push_string("/data/out.txt");
        msg.push_ulong(1024);

        let mut resp = Message::from_data(msg.get_data().clone());
        let fields = parse_upload_fields(&mut resp);

        assert_eq!(fields.uuid, "uuid-123");
        assert_eq!(fields.job_id, 42);
        assert_eq!(fields.bundle_hash, "bundle-hash");
        assert_eq!(fields.target_path, "/data/out.txt");
        assert_eq!(fields.file_size, 1024);
    }

    struct TestSink {
        sent: Vec<WsMessage>,
        fail_next: bool,
    }

    impl TestSink {
        fn new() -> Self {
            Self {
                sent: Vec::new(),
                fail_next: false,
            }
        }
    }

    impl Sink<WsMessage> for TestSink {
        type Error = std::io::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(self: std::pin::Pin<&mut Self>, item: WsMessage) -> Result<(), Self::Error> {
            let this = self.get_mut();
            if this.fail_next {
                return Err(std::io::Error::other("mock send failure"));
            }
            this.sent.push(item);
            Ok(())
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn test_send_file_error_sends_message() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut sink = TestSink::new();
            send_file_error(&mut sink, "uuid-123", "boom", FILE_DOWNLOAD_ERROR).await;

            assert_eq!(sink.sent.len(), 1);
            let WsMessage::Binary(data) = &sink.sent[0] else {
                panic!("expected a binary message");
            };
            let mut resp = Message::from_data(data.to_vec());
            assert_eq!(resp.id, FILE_DOWNLOAD_ERROR);
            assert_eq!(resp.source, "uuid-123");
            assert_eq!(resp.pop_string(), "boom");
        });
    }

    #[test]
    fn test_send_file_error_send_failure_does_not_panic() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut sink = TestSink::new();
            sink.fail_next = true;
            send_file_error(&mut sink, "uuid-123", "boom", FILE_UPLOAD_ERROR).await;

            assert!(sink.sent.is_empty());
        });
    }

    struct FailingFinalizeFile {
        fail_flush: bool,
        fail_sync_all: bool,
    }

    impl FinalizeFile for FailingFinalizeFile {
        async fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                Err(std::io::Error::other("mock flush failure"))
            } else {
                Ok(())
            }
        }

        async fn sync_all(&mut self) -> std::io::Result<()> {
            if self.fail_sync_all {
                Err(std::io::Error::other("mock sync failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn test_finalize_upload_file_success_keeps_file_and_sends_no_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = TempDir::new().unwrap();
            let full_path = tmp.path().join("upload.bin");
            std::fs::write(&full_path, b"data").unwrap();

            let mut file = FailingFinalizeFile {
                fail_flush: false,
                fail_sync_all: false,
            };
            let mut sink = TestSink::new();

            let ok = finalize_upload_file(&mut file, &full_path, &mut sink, "uuid-123").await;

            assert!(ok);
            assert!(full_path.exists(), "successful finalize keeps the file");
            assert!(sink.sent.is_empty());
        });
    }

    #[test]
    fn test_finalize_upload_file_flush_failure_removes_file_and_sends_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = TempDir::new().unwrap();
            let full_path = tmp.path().join("upload.bin");
            std::fs::write(&full_path, b"data").unwrap();

            let mut file = FailingFinalizeFile {
                fail_flush: true,
                fail_sync_all: false,
            };
            let mut sink = TestSink::new();

            let ok = finalize_upload_file(&mut file, &full_path, &mut sink, "uuid-123").await;

            assert!(!ok);
            assert!(
                !full_path.exists(),
                "partial file should be removed on flush failure"
            );
            assert_eq!(sink.sent.len(), 1);
            let WsMessage::Binary(data) = &sink.sent[0] else {
                panic!("expected a binary message");
            };
            let mut resp = Message::from_data(data.to_vec());
            assert_eq!(resp.id, FILE_UPLOAD_ERROR);
            assert_eq!(resp.source, "uuid-123");
            assert_eq!(resp.pop_string(), "Failed to finalize uploaded file");
        });
    }

    #[test]
    fn test_finalize_upload_file_sync_all_failure_removes_file_and_sends_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = TempDir::new().unwrap();
            let full_path = tmp.path().join("upload.bin");
            std::fs::write(&full_path, b"data").unwrap();

            let mut file = FailingFinalizeFile {
                fail_flush: false,
                fail_sync_all: true,
            };
            let mut sink = TestSink::new();

            let ok = finalize_upload_file(&mut file, &full_path, &mut sink, "uuid-123").await;

            assert!(!ok);
            assert!(
                !full_path.exists(),
                "partial file should be removed on sync failure"
            );
            assert_eq!(sink.sent.len(), 1);
            let WsMessage::Binary(data) = &sink.sent[0] else {
                panic!("expected a binary message");
            };
            let mut resp = Message::from_data(data.to_vec());
            assert_eq!(resp.id, FILE_UPLOAD_ERROR);
            assert_eq!(resp.source, "uuid-123");
            assert_eq!(resp.pop_string(), "Failed to finalize uploaded file");
        });
    }

    #[test]
    fn lookup_job_working_directory_returns_dir_for_submitted_job() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let resp = make_job_lookup_response(false, "/data/workdir");
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = rt.block_on(async { lookup_job_working_directory(22).await });

        match dir {
            Ok(dir) => assert_eq!(dir, "/data/workdir"),
            Err(err) => panic!("expected working directory, got: {}", err.client_message()),
        }
    }

    #[test]
    fn lookup_job_working_directory_rejects_submitting_job() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let resp = make_job_lookup_response(true, "/data/workdir");
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(async { lookup_job_working_directory(22).await })
            .unwrap_err();

        assert!(matches!(err, JobLookupError::NotSubmitted));
        assert_eq!(err.client_message(), "Job is not submitted");
    }

    #[test]
    fn lookup_job_working_directory_reports_missing_job() {
        let _guard = TEST_MUTEX.lock().unwrap();
        reset_websocket_client_for_test();
        let mut mock = MockWebsocketClient::new();
        mock.expect_send_db_request().times(1).returning(|_| {
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_uint(0);
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(async { lookup_job_working_directory(22).await })
            .unwrap_err();

        assert!(matches!(err, JobLookupError::NotFound));
        assert_eq!(err.client_message(), "Job does not exist");
    }

    #[test]
    fn lookup_job_working_directory_maps_db_error() {
        let _guard = TEST_MUTEX.lock().unwrap();
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
        let err = rt
            .block_on(async { lookup_job_working_directory(22).await })
            .unwrap_err();

        assert!(matches!(err, JobLookupError::Database(_)));
        assert_eq!(err.client_message(), "Database error: db connection failed");
    }

    #[tokio::test]
    async fn test_send_file_error_sends_file_download_error() {
        let server = WebsocketServerFixture::new().await;
        let url = format!("ws://127.0.0.1:{}/ws/", server.port);
        let request = build_file_ws_request(&url, "test-token").unwrap();
        let (ws_stream, _) = connect_async(request).await.unwrap();
        let (mut ws_sender, _ws_receiver) = ws_stream.split();

        let test_uuid = "test-uuid-download-error";
        send_file_error(
            &mut ws_sender,
            test_uuid,
            "Exception reading file",
            FILE_DOWNLOAD_ERROR,
        )
        .await;

        let mut server = server;
        let response = tokio::time::timeout(Duration::from_secs(2), server.msg_rx.recv())
            .await
            .expect("Timeout waiting for FILE_DOWNLOAD_ERROR")
            .expect("No response");

        assert_eq!(response.id, FILE_DOWNLOAD_ERROR);
        let mut response_msg = response;
        assert_eq!(response_msg.pop_string(), "Exception reading file");
        assert_eq!(response_msg.source, test_uuid);
    }
}
