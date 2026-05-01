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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use tracing::error;

pub async fn handle_file_list(mut msg: Message) {
    let job_id = i64::from(msg.pop_uint());
    let uuid = msg.pop_string();
    let bundle_hash = msg.pop_string();
    let dir_path = msg.pop_string();
    let is_recursive = msg.pop_bool();

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
        let bm = BundleManager::singleton();
        bm.run_bundle_string(
            "working_directory",
            &bundle_hash,
            &json!(dir_path),
            "file_list",
        )
    };

    let full_path = Path::new(&working_directory).join(&dir_path);
    let Ok(abs_path) = fs::canonicalize(&full_path).await else {
        send_file_list_error(&uuid, "Path to list files does not exist");
        return;
    };

    let Ok(canonical_working) = fs::canonicalize(&working_directory).await else {
        send_file_list_error(&uuid, "Path to list files is outside the working directory");
        return;
    };

    if !abs_path.starts_with(&canonical_working) {
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
                    let metadata = entry.metadata().await.unwrap();
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
            let metadata = entry.metadata().await.unwrap();
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
    result.push_uint(u32::try_from(file_list.len()).unwrap_or(u32::MAX));
    for (path, is_dir, size) in file_list {
        result.push_string(&path);
        result.push_bool(is_dir);
        result.push_ulong(size);
    }
    get_websocket_client().queue_message(
        uuid,
        result.get_data().clone(),
        Priority::Highest,
        Arc::new(|| {}),
    );
}

fn send_file_list_error(uuid: &str, error_msg: &str) {
    let mut result = Message::new(FILE_LIST_ERROR, Priority::Highest, uuid);
    result.push_string(uuid);
    result.push_string(error_msg);
    get_websocket_client().queue_message(
        uuid.to_string(),
        result.get_data().clone(),
        Priority::Highest,
        Arc::new(|| {}),
    );
}

pub fn handle_file_download(mut msg: Message) {
    let job_id = i64::from(msg.pop_uint());
    let uuid = msg.pop_string();
    let bundle_hash = msg.pop_string();
    let mut file_path = msg.pop_string();

    tokio::spawn(async move {
        let config = crate::config::read_client_config();
        let ws_endpoint = config["websocketEndpoint"]
            .as_str()
            .unwrap_or("ws://127.0.0.1:8001/ws/");
        let url = format!("{ws_endpoint}?token={uuid}");

        let (ws_stream, _) = match connect_async(&url).await {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to connect for file download: {}", e);
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        if let Some(Ok(WsMessage::Binary(data))) = ws_receiver.next().await {
            let msg = Message::from_data(data.to_vec());
            if msg.id != SERVER_READY {
                error!("Expected SERVER_READY, got {}", msg.id);
                return;
            }
        }

        let working_directory = if job_id != 0 {
            if let Ok(Some(job)) = db::get_job_by_job_id(job_id).await {
                if job.submitting {
                    send_download_error(&mut ws_sender, &uuid, "Job is not submitted").await;
                    return;
                }
                job.working_directory
            } else {
                send_download_error(&mut ws_sender, &uuid, "Job does not exist").await;
                return;
            }
        } else {
            let bm = BundleManager::singleton();
            bm.run_bundle_string(
                "working_directory",
                &bundle_hash,
                &json!(file_path),
                "file_download",
            )
        };

        while file_path.starts_with('/') {
            file_path.remove(0);
        }

        let full_path = Path::new(&working_directory).join(&file_path);
        let Ok(abs_path) = fs::canonicalize(&full_path).await else {
            send_download_error(
                &mut ws_sender,
                &uuid,
                "Path to file download does not exist",
            )
            .await;
            return;
        };

        let canonical_working = fs::canonicalize(&working_directory)
            .await
            .unwrap_or_else(|_| PathBuf::from(&working_directory));
        if !abs_path.starts_with(&canonical_working) {
            send_download_error(
                &mut ws_sender,
                &uuid,
                "Path to file download is outside the working directory",
            )
            .await;
            return;
        }

        if !fs::metadata(&abs_path).await.is_ok_and(|m| m.is_file()) {
            send_download_error(&mut ws_sender, &uuid, "Path to file download is not a file").await;
            return;
        }

        let file_size = fs::metadata(&abs_path).await.unwrap().len();
        let mut details_msg = Message::new(FILE_DOWNLOAD_DETAILS, Priority::Highest, &uuid);
        details_msg.push_ulong(file_size);
        let _ = ws_sender
            .send(WsMessage::Binary(details_msg.get_data().clone().into()))
            .await;

        let mut file = File::open(&abs_path).await.unwrap();
        let mut buffer = vec![0u8; 64 * 1024];

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
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    error!("Error reading file: {}", e);
                    let mut err_msg = Message::new(FILE_DOWNLOAD_ERROR, Priority::Highest, &uuid);
                    err_msg.push_string("Exception reading file");
                    let _ = ws_sender
                        .send(WsMessage::Binary(err_msg.get_data().clone().into()))
                        .await;
                    return;
                }
            };

            let mut chunk_msg = Message::new(FILE_CHUNK, Priority::Highest, &uuid);
            chunk_msg.push_bytes(&buffer[..n]);
            if ws_sender
                .send(WsMessage::Binary(chunk_msg.get_data().clone().into()))
                .await
                .is_err()
            {
                break;
            }
            // Yield to allow pause/resume messages to be processed
            tokio::task::yield_now().await;
        }
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
    let url = format!("{ws_endpoint}?token={uuid}");

    handle_file_upload_internal(uuid, job_id, bundle_hash, target_path, file_size, url);
}

pub fn handle_file_upload_with_url(mut msg: Message, url: String) {
    let uuid = msg.pop_string();
    let job_id = i64::from(msg.pop_uint());
    let bundle_hash = msg.pop_string();
    let target_path = msg.pop_string();
    let file_size = msg.pop_ulong();

    handle_file_upload_internal(uuid, job_id, bundle_hash, target_path, file_size, url);
}

fn handle_file_upload_internal(
    uuid: String,
    job_id: i64,
    bundle_hash: String,
    mut target_path: String,
    file_size: u64,
    url: String,
) {
    tokio::spawn(async move {
        let (ws_stream, _) = match connect_async(&url).await {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to connect for file upload: {}", e);
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        if let Some(Ok(WsMessage::Binary(data))) = ws_receiver.next().await {
            let msg = Message::from_data(data.to_vec());
            if msg.id != SERVER_READY {
                error!("Expected SERVER_READY, got {}", msg.id);
                return;
            }
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
            let bm = BundleManager::singleton();
            bm.run_bundle_string(
                "working_directory",
                &bundle_hash,
                &json!(target_path),
                "file_upload",
            )
        };

        while target_path.starts_with('/') {
            target_path.remove(0);
        }

        let full_path = Path::new(&working_directory).join(&target_path);

        let canonical_working = fs::canonicalize(&working_directory)
            .await
            .unwrap_or_else(|_| PathBuf::from(&working_directory));

        // Canonicalize as much of the path as exists to detect symlink escapes
        // First, find the longest existing prefix of the path
        let mut existing_prefix = PathBuf::new();
        let mut remaining_suffix = PathBuf::new();
        for component in full_path.components() {
            let test_path = if existing_prefix.as_os_str().is_empty() {
                PathBuf::from(component.as_os_str())
            } else {
                existing_prefix.join(component.as_os_str())
            };
            if test_path.exists() {
                existing_prefix = test_path;
                remaining_suffix = PathBuf::new();
            } else if remaining_suffix.as_os_str().is_empty() {
                remaining_suffix = PathBuf::from(component.as_os_str());
            } else {
                remaining_suffix = remaining_suffix.join(component.as_os_str());
            }
        }

        // Canonicalize the existing prefix to resolve any symlinks
        let abs_path = if existing_prefix.as_os_str().is_empty() {
            full_path.clone()
        } else {
            match fs::canonicalize(&existing_prefix).await {
                Ok(canonical_prefix) => {
                    if remaining_suffix.as_os_str().is_empty() {
                        canonical_prefix
                    } else {
                        canonical_prefix.join(remaining_suffix)
                    }
                }
                Err(_) => full_path.clone(),
            }
        };

        if !abs_path
            .to_string_lossy()
            .starts_with(&canonical_working.to_string_lossy().into_owned())
        {
            send_upload_error(
                &mut ws_sender,
                &uuid,
                "Target path for file upload is outside the working directory",
            )
            .await;
            return;
        }

        // Now create parent directories if needed
        if let Some(parent) = full_path.parent() {
            let _ = fs::create_dir_all(parent).await;
        }

        let mut file = match File::create(&abs_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create file: {}", e);
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
        while let Some(Ok(ws_msg)) = ws_receiver.next().await {
            if let WsMessage::Binary(data) = ws_msg {
                let mut m = Message::from_data(data.to_vec());
                if m.id == FILE_UPLOAD_CHUNK {
                    let chunk = m.pop_bytes();
                    if let Err(e) = file.write_all(&chunk).await {
                        error!("Failed to write chunk: {}", e);
                        let _ = fs::remove_file(&abs_path).await;
                        send_upload_error(&mut ws_sender, &uuid, "Failed to write chunk to file")
                            .await;
                        return;
                    }
                    received_size += chunk.len() as u64;
                } else if m.id == FILE_UPLOAD_COMPLETE {
                    if received_size != file_size {
                        let _ = fs::remove_file(&abs_path).await;
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
                        error!("Failed to flush uploaded file: {}", e);
                        let _ = fs::remove_file(&abs_path).await;
                        send_upload_error(
                            &mut ws_sender,
                            &uuid,
                            "Failed to finalize uploaded file",
                        )
                        .await;
                        return;
                    }
                    if let Err(e) = file.sync_all().await {
                        error!("Failed to sync uploaded file: {}", e);
                        let _ = fs::remove_file(&abs_path).await;
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
                    return;
                }
            }
        }
    });
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
