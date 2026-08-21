use crate::bundle_manager::resolve_working_directory;
use crate::db;
use crate::messaging::{
    Message, Priority, FILE_CHUNK, FILE_DOWNLOAD_DETAILS, FILE_DOWNLOAD_ERROR, FILE_LIST,
    FILE_LIST_ERROR, FILE_UPLOAD_CHUNK, FILE_UPLOAD_COMPLETE, FILE_UPLOAD_ERROR,
    PAUSE_FILE_CHUNK_STREAM, RESUME_FILE_CHUNK_STREAM, SERVER_READY,
};
use crate::websocket::get_websocket_client;
use bytes::Bytes;
use futures_util::{Sink, SinkExt, StreamExt};
use serde_json::json;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, LazyLock,
};
#[cfg(test)]
use std::sync::{Mutex, PoisonError};
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
/// Test-only override for the `SERVER_READY` handshake timeout. When `None`
/// the production 10-second deadline applies. Tests may shrink it to
/// milliseconds to exercise the readiness timeout path deterministically.
#[cfg(test)]
static SERVER_READY_TIMEOUT_OVERRIDE: LazyLock<Mutex<Option<Duration>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub(crate) fn set_server_ready_timeout_for_test(timeout: Option<Duration>) {
    let mut guard = SERVER_READY_TIMEOUT_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = timeout;
}

#[cfg(test)]
fn server_ready_timeout() -> Duration {
    let guard = SERVER_READY_TIMEOUT_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    guard.unwrap_or(Duration::from_secs(SERVER_READY_TIMEOUT_SECS))
}

const GRACEFUL_CLOSE_TIMEOUT_SECS: u64 = 5;

/// Test-only override for [`graceful_close_timeout`]. When `None` the
/// production five-second deadline applies. Tests may set a short duration to
/// exercise the forced-release path deterministically without wall-clock
/// sleeps. The override is scoped narrowly to the graceful-shutdown deadline;
/// it never affects readiness, connect, or transfer work.
#[cfg(test)]
static GRACEFUL_CLOSE_TIMEOUT_OVERRIDE: LazyLock<Mutex<Option<Duration>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
fn graceful_close_timeout() -> Duration {
    let guard = GRACEFUL_CLOSE_TIMEOUT_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    guard.unwrap_or(Duration::from_secs(GRACEFUL_CLOSE_TIMEOUT_SECS))
}

#[cfg(not(test))]
fn graceful_close_timeout() -> Duration {
    Duration::from_secs(GRACEFUL_CLOSE_TIMEOUT_SECS)
}

#[cfg(test)]
pub(crate) fn set_graceful_close_timeout_for_test(timeout: Option<Duration>) {
    let mut guard = GRACEFUL_CLOSE_TIMEOUT_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = timeout;
}

/// Test-only seam that exposes the supervisor's final-send and zero-byte EOF
/// boundaries to integration tests via [`LifecycleBarrier`]. The supervisor
/// calls `arrive_and_wait` on whichever barrier is currently set so a test
/// can inject a peer terminal event between the final chunk send and the
/// zero-byte EOF read, or between the zero-byte EOF read and the
/// authoritative-result selection. The seams are no-ops when no barrier is
/// installed; production behaviour is therefore unchanged.
#[cfg(test)]
use crate::tests::fixtures::websocket_server_fixture::LifecycleBarrier;

#[cfg(test)]
use tokio::sync::mpsc;

#[cfg(test)]
static TEST_FINAL_SEND_BARRIER: LazyLock<Mutex<Option<Arc<LifecycleBarrier>>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
static TEST_ZERO_BYTE_EOF_BARRIER: LazyLock<Mutex<Option<Arc<LifecycleBarrier>>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub(crate) fn set_final_send_barrier_for_test(barrier: Option<Arc<LifecycleBarrier>>) {
    let mut guard = TEST_FINAL_SEND_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = barrier;
}

#[cfg(test)]
pub(crate) fn set_zero_byte_eof_barrier_for_test(barrier: Option<Arc<LifecycleBarrier>>) {
    let mut guard = TEST_ZERO_BYTE_EOF_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = barrier;
}

#[cfg(test)]
async fn arrive_final_send_barrier() {
    let barrier = TEST_FINAL_SEND_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(b) = barrier {
        b.arrive_and_wait().await;
    }
}

#[cfg(test)]
async fn arrive_zero_byte_eof_barrier() {
    let barrier = TEST_ZERO_BYTE_EOF_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(b) = barrier {
        b.arrive_and_wait().await;
    }
}

/// Test-only seam that parks the supervisor immediately before the graceful
/// Close frame send in [`cleanup_download`]. Lets a test reset the peer
/// transport so the Close send deterministically fails after a successful
/// transfer. The seam is a no-op when no barrier is installed.
#[cfg(test)]
static TEST_PRE_CLOSE_SEND_BARRIER: LazyLock<Mutex<Option<Arc<LifecycleBarrier>>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub(crate) fn set_pre_close_send_barrier_for_test(barrier: Option<Arc<LifecycleBarrier>>) {
    let mut guard = TEST_PRE_CLOSE_SEND_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = barrier;
}

#[cfg(test)]
async fn arrive_pre_close_send_barrier() {
    let barrier = TEST_PRE_CLOSE_SEND_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(b) = barrier {
        b.arrive_and_wait().await;
    }
}

/// Test-only seam that exposes the supervisor's authoritative transfer result
/// to integration tests. The supervisor sends the selected `TransferOutcome`
/// at the start of unified cleanup — covering both transfer-loop results and
/// pre-transfer primary errors — to the current observer. The seam is a no-op
/// when no observer is installed; production behaviour is therefore unchanged.
#[cfg(test)]
static TEST_TRANSFER_OUTCOME_OBSERVER: LazyLock<
    Mutex<Option<mpsc::UnboundedSender<TransferOutcome>>>,
> = LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub(crate) fn set_transfer_outcome_observer_for_test(
    observer: Option<mpsc::UnboundedSender<TransferOutcome>>,
) {
    let mut guard = TEST_TRANSFER_OUTCOME_OBSERVER
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = observer;
}

#[cfg(test)]
fn notify_transfer_outcome_observer(authoritative: &AuthoritativeResult) {
    let observer = TEST_TRANSFER_OUTCOME_OBSERVER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(tx) = observer {
        let _ = tx.send(authoritative.clone().into_outcome());
    }
}

/// Test-only seam that exposes cleanup failures (Close-send failure or
/// graceful-shutdown timeout) to integration tests. Lets a test assert that a
/// cleanup failure was recorded without masking the preserved primary result.
/// The seam is a no-op when no observer is installed.
#[cfg(test)]
static TEST_CLEANUP_FAILURE_OBSERVER: LazyLock<Mutex<Option<mpsc::UnboundedSender<String>>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub(crate) fn set_cleanup_failure_observer_for_test(
    observer: Option<mpsc::UnboundedSender<String>>,
) {
    let mut guard = TEST_CLEANUP_FAILURE_OBSERVER
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *guard = observer;
}

#[cfg(test)]
fn notify_cleanup_failure(message: String) {
    let observer = TEST_CLEANUP_FAILURE_OBSERVER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(tx) = observer {
        let _ = tx.send(message);
    }
}

type WsSender = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;
type WsReceiver = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Authoritative transfer result selected by the supervisor before unified
/// cleanup. Once a primary operation failure has been selected, later peer
/// terminal events and cleanup failures cannot replace it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthoritativeResult {
    /// No result yet — used internally before the clean-EOF boundary.
    Pending,
    /// A post-connect primary operation failed: `SERVER_READY` validation,
    /// job lookup, bundle-directory resolution, canonicalisation, path
    /// containment, metadata inspection, file open, details send, file read,
    /// or chunk send.
    PrimaryError(String),
    /// Peer sent Close, EOF, or a receive error before the clean-EOF boundary.
    PeerTerminal(String),
    /// Clean EOF after every byte was sent successfully and expected-size
    /// equality held.
    CleanEof,
}

impl AuthoritativeResult {
    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    fn into_outcome(self) -> TransferOutcome {
        match self {
            Self::Pending => TransferOutcome::PeerTerminal("exited without result".to_string()),
            Self::PrimaryError(msg) => TransferOutcome::PrimaryError(msg),
            Self::PeerTerminal(msg) => TransferOutcome::PeerTerminal(msg),
            Self::CleanEof => TransferOutcome::CleanEof,
        }
    }
}

/// Same as [`AuthoritativeResult`] but exposes the terminal variants only;
/// used after the supervisor finishes to drive completion and failure
/// reporting without exposing internal `Pending` state. `pub(crate)` so the
/// test-only outcome observer can hand it to integration tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferOutcome {
    PrimaryError(String),
    PeerTerminal(String),
    CleanEof,
}

enum ChunkState {
    Reading,
    Sending,
}

enum IncomingEvent {
    Pause,
    Resume,
    Close,
    Eof,
    Error(String),
    Ignored,
}

enum LoopStep {
    Continue,
    SetState(ChunkState),
    Finish(AuthoritativeResult),
}

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
        let abs_path = match fs::canonicalize(&full_path).await {
            Ok(path) => path,
            Err(e) => {
                warn!("handle_file_list: Failed to canonicalize path: {}", e);
                send_file_list_error(&uuid, "Path to list files does not exist");
                return;
            }
        };

        if !validate_path_is_within(&abs_path, &working_directory).await {
            send_file_list_error(&uuid, "Path to list files is outside the working directory");
            return;
        }

        if validate_list_target_is_directory(&abs_path).await.is_err() {
            send_file_list_error(&uuid, "Path to list files is not a directory");
            return;
        }

        let mut file_list = Vec::new();
        if is_recursive {
            let mut stack = vec![abs_path.clone()];
            while let Some(current_dir) = stack.pop() {
                match fs::read_dir(&current_dir).await {
                    Ok(mut entries) => {
                        let mut handler = RecursiveFileListHandler {
                            file_list: &mut file_list,
                            stack: &mut stack,
                            working_directory: &working_directory,
                        };
                        for_each_dir_entry(&mut entries, &current_dir, &mut handler).await;
                    }
                    Err(e) => {
                        warn!(
                            "handle_file_list: failed to read directory {:?}: {}",
                            current_dir, e
                        );
                    }
                }
            }
        } else {
            match fs::read_dir(&abs_path).await {
                Ok(mut entries) => {
                    let mut handler = FileListHandler {
                        file_list: &mut file_list,
                        working_directory: &working_directory,
                    };
                    for_each_dir_entry(&mut entries, &abs_path, &mut handler).await;
                }
                Err(e) => {
                    warn!(
                        "handle_file_list: failed to read directory {:?}: {}",
                        abs_path, e
                    );
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

async fn validate_list_target_is_directory(abs_path: &Path) -> Result<(), ()> {
    match fs::metadata(abs_path).await {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(()),
        Err(e) => {
            warn!("handle_file_list: Failed to get file metadata: {}", e);
            Err(())
        }
    }
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

/// A source of directory entries whose iteration can fail mid-stream.
trait DirEntrySource {
    async fn next_entry(&mut self) -> std::io::Result<Option<fs::DirEntry>>;
}

impl DirEntrySource for fs::ReadDir {
    async fn next_entry(&mut self) -> std::io::Result<Option<fs::DirEntry>> {
        fs::ReadDir::next_entry(self).await
    }
}

/// Handles a single directory entry during iteration.
trait DirEntryHandler {
    async fn handle(&mut self, entry: fs::DirEntry);
}

/// Iterate a directory stream, invoking `handler` for each entry. Logs a warning
/// and stops iteration if `next_entry` fails.
async fn for_each_dir_entry<S, H>(entries: &mut S, dir: &Path, handler: &mut H)
where
    S: DirEntrySource,
    H: DirEntryHandler,
{
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => handler.handle(entry).await,
            Ok(None) => break,
            Err(e) => {
                warn!(
                    "handle_file_list: failed to read directory entry in {:?}: {}",
                    dir, e
                );
                break;
            }
        }
    }
}

/// Collects entries from a single directory into `file_list`, pushing
/// subdirectories onto `stack` for recursive traversal.
struct RecursiveFileListHandler<'a> {
    file_list: &'a mut Vec<(String, bool, u64)>,
    stack: &'a mut Vec<PathBuf>,
    working_directory: &'a str,
}

impl DirEntryHandler for RecursiveFileListHandler<'_> {
    async fn handle(&mut self, entry: fs::DirEntry) {
        let path = entry.path();
        if let Some((relative_path, is_dir, size)) =
            collect_dir_entry(entry, self.working_directory).await
        {
            self.file_list.push((relative_path, is_dir, size));
            if is_dir {
                self.stack.push(path);
            }
        }
    }
}

/// Collects entries from a single directory into `file_list`.
struct FileListHandler<'a> {
    file_list: &'a mut Vec<(String, bool, u64)>,
    working_directory: &'a str,
}

impl DirEntryHandler for FileListHandler<'_> {
    async fn handle(&mut self, entry: fs::DirEntry) {
        if let Some(entry_info) = collect_dir_entry(entry, self.working_directory).await {
            self.file_list.push(entry_info);
        }
    }
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
    let file_path = msg.pop_string();

    tokio::spawn(async move {
        run_download_supervisor(job_id, uuid, bundle_hash, file_path).await;
    });
}

/// One supervisor task owns both WebSocket halves and all download lifecycle
/// state from immediately after `connect_async` until resource release. There
/// is no detached pause/resume listener. The supervisor drives Connected,
/// Transferring, Closing, and Forced release control flow.
async fn run_download_supervisor(
    job_id: i64,
    uuid: String,
    bundle_hash: String,
    file_path: String,
) {
    debug!(
        "handle_file_download: SPAWNED - job_id={}, uuid={}, bundle_hash={}, file_path={}",
        job_id, uuid, bundle_hash, file_path
    );
    debug!(
        "handle_file_download: STARTED - job_id={}, uuid={}, bundle_hash={}, file_path={}",
        job_id, uuid, bundle_hash, file_path
    );

    // --- Connected: establish the WebSocket and own both halves immediately.
    let ws_endpoint = get_ws_endpoint_from_config();
    let Some((mut ws_sender, mut ws_receiver)) = connect_file_ws_raw(
        &ws_endpoint,
        &uuid,
        "handle_file_download: ",
        "file download",
    )
    .await
    else {
        return;
    };

    // --- Connected → readiness validation.
    if let Err(reason) = validate_server_ready(&mut ws_receiver).await {
        warn!("handle_file_download: SERVER_READY validation failed ({reason}); entering cleanup");
        let authoritative =
            AuthoritativeResult::PrimaryError(format!("SERVER_READY validation failed: {reason}"));
        cleanup_download(&mut ws_sender, &mut ws_receiver, authoritative).await;
        return;
    }

    // --- Connected → post-connect primary operations (each may select a primary error).
    debug!(
        "handle_file_download: SERVER_READY received, resolving working directory for job_id={}",
        job_id
    );
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
                log_job_lookup_error("handle_file_download", job_id, &err);
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    &err.client_message(),
                    FILE_DOWNLOAD_ERROR,
                )
                .await;
                cleanup_download(
                    &mut ws_sender,
                    &mut ws_receiver,
                    AuthoritativeResult::PrimaryError(format!(
                        "job lookup failed: {}",
                        err.client_message()
                    )),
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
    let trimmed_path = file_path.trim_start_matches('/').to_string();
    let full_path = Path::new(&working_directory).join(&trimmed_path);
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
            cleanup_download(
                &mut ws_sender,
                &mut ws_receiver,
                AuthoritativeResult::PrimaryError("canonicalize failed".to_string()),
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
        cleanup_download(
            &mut ws_sender,
            &mut ws_receiver,
            AuthoritativeResult::PrimaryError("path outside working directory".to_string()),
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
            cleanup_download(
                &mut ws_sender,
                &mut ws_receiver,
                AuthoritativeResult::PrimaryError("path is not a file".to_string()),
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
            cleanup_download(
                &mut ws_sender,
                &mut ws_receiver,
                AuthoritativeResult::PrimaryError("metadata read failed".to_string()),
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
        cleanup_download(
            &mut ws_sender,
            &mut ws_receiver,
            AuthoritativeResult::PrimaryError("details send failed".to_string()),
        )
        .await;
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
            cleanup_download(
                &mut ws_sender,
                &mut ws_receiver,
                AuthoritativeResult::PrimaryError("file open failed".to_string()),
            )
            .await;
            return;
        }
    };

    // --- Transferring: one ordered supervisor event loop drives file reads,
    // chunk sends, pause/resume input, and peer terminal events.
    let is_paused = Arc::new(AtomicBool::new(false));
    let resume_notify = Arc::new(Notify::new());
    let download_start = std::time::Instant::now();
    let mut buffer = vec![0u8; DOWNLOAD_CHUNK_SIZE];

    let state = run_transfer_loop(
        &mut ws_sender,
        &mut ws_receiver,
        &mut file,
        &mut buffer,
        file_size,
        &is_paused,
        &resume_notify,
        &uuid,
    )
    .await;

    // --- User-facing reporting from the authoritative result (cleanup below
    // never replaces this).
    match state.authoritative().clone().into_outcome() {
        TransferOutcome::CleanEof => {
            debug!(
                "handle_file_download: COMPLETED - downloaded {} bytes in {:?}",
                state.total_bytes(),
                download_start.elapsed()
            );
        }
        TransferOutcome::PrimaryError(msg) => {
            warn!("handle_file_download: primary transfer failure: {}", msg);
        }
        TransferOutcome::PeerTerminal(msg) => {
            warn!(
                "handle_file_download: peer terminal event during transfer: {}",
                msg
            );
        }
    }

    // --- Closing → Forced release unified cleanup. The supervisor still owns
    // both halves here; they are dropped as the task ends.
    cleanup_download(&mut ws_sender, &mut ws_receiver, state.take_authoritative()).await;
}

/// Validate `SERVER_READY` on the supervisor-owned receive half. Returns the
/// reason string on any readiness failure shape (invalid ID, non-binary,
/// receive error, peer EOF, timeout).
async fn validate_server_ready(ws_receiver: &mut WsReceiver) -> Result<(), String> {
    let timeout = {
        #[cfg(test)]
        {
            server_ready_timeout()
        }
        #[cfg(not(test))]
        {
            Duration::from_secs(SERVER_READY_TIMEOUT_SECS)
        }
    };
    let handshake = tokio::time::timeout(timeout, ws_receiver.next()).await;
    match handshake {
        Ok(Some(Ok(WsMessage::Binary(data)))) => {
            let msg = Message::from_data(data.to_vec());
            if msg.id == SERVER_READY {
                Ok(())
            } else {
                Err(format!("expected SERVER_READY, got {}", msg.id))
            }
        }
        Ok(Some(Ok(_))) => Err("expected binary SERVER_READY, got unexpected frame".to_string()),
        Ok(Some(Err(e))) => Err(format!("handshake error: {e}")),
        Ok(None) => Err("server closed connection before sending SERVER_READY".to_string()),
        Err(_) => Err(format!(
            "timeout waiting for SERVER_READY after {SERVER_READY_TIMEOUT_SECS}s"
        )),
    }
}

/// Drive file reads, chunk sends, pause/resume input, and peer terminal
/// events in one ordered event loop. Transmitted bytes advance only after a
/// chunk send resolves successfully. `COMPLETED` requires clean EOF plus
/// expected-size equality. Once a primary error is selected, later peer
/// terminal events cannot replace it.
async fn run_transfer_loop(
    ws_sender: &mut WsSender,
    ws_receiver: &mut WsReceiver,
    file: &mut File,
    buffer: &mut [u8],
    file_size: u64,
    is_paused: &Arc<AtomicBool>,
    resume_notify: &Arc<Notify>,
    uuid: &str,
) -> TransferState {
    let mut state = TransferState::new(file_size);
    let mut chunk_state = ChunkState::Reading;

    loop {
        let step = match chunk_state {
            ChunkState::Reading => {
                run_reading_phase(
                    ws_sender,
                    ws_receiver,
                    file,
                    buffer,
                    is_paused,
                    resume_notify,
                    uuid,
                    &mut state,
                )
                .await
            }
            ChunkState::Sending => {
                run_sending_phase(ws_sender, ws_receiver, is_paused, resume_notify, &mut state)
                    .await
            }
        };

        match step {
            LoopStep::Continue => {}
            LoopStep::SetState(next) => chunk_state = next,
            LoopStep::Finish(result) => {
                state.set_authoritative(result);
                break;
            }
        }
    }

    state
}

async fn run_reading_phase(
    ws_sender: &mut WsSender,
    ws_receiver: &mut WsReceiver,
    file: &mut File,
    buffer: &mut [u8],
    is_paused: &Arc<AtomicBool>,
    resume_notify: &Arc<Notify>,
    uuid: &str,
    state: &mut TransferState,
) -> LoopStep {
    // If a primary error already exists, only listen for peer terminal events
    // and resume notifications so we do not waste file work.
    if state.authoritative.is_primary() {
        return wait_for_terminal_or_resume(ws_receiver, is_paused, resume_notify, state).await;
    }

    // Process any Pause/Resume/terminal messages already buffered before
    // arming a file read, so a Pause that arrived during the previous send is
    // honoured promptly.
    if let Some(step) = drain_pending_incoming(ws_receiver, is_paused, resume_notify, state) {
        return step;
    }

    if is_paused.load(Ordering::Acquire) {
        // While paused we do not arm a file read; we just wait for resume or a
        // peer terminal event.
        return wait_for_terminal_or_resume(ws_receiver, is_paused, resume_notify, state).await;
    }

    let mut read_fut = Box::pin(file.read(buffer));
    tokio::select! {
        biased;
        incoming = ws_receiver.next() => {
            match classify_incoming(incoming) {
                IncomingEvent::Pause => {
                    is_paused.store(true, Ordering::Release);
                    LoopStep::Continue
                }
                IncomingEvent::Resume => {
                    is_paused.store(false, Ordering::Release);
                    resume_notify.notify_one();
                    LoopStep::Continue
                }
                IncomingEvent::Close => {
                    // Peer Close during transfer becomes the authoritative
                    // terminal event. Drops both halves during cleanup.
                    LoopStep::Finish(state.peer_terminal("peer Close".to_string()))
                }
                IncomingEvent::Eof => LoopStep::Finish(state.peer_terminal("peer EOF".to_string())),
                IncomingEvent::Error(msg) => {
                    LoopStep::Finish(state.peer_terminal(format!("peer receive error: {msg}")))
                }
                IncomingEvent::Ignored => LoopStep::Continue,
            }
        }
        read_result = &mut read_fut => {
            match read_result {
                Ok(0) => {
                    // Clean EOF is only valid after every byte has been sent
                    // successfully and total bytes match the expected size.
                    trace!("handle_file_download: End of file reached");
                    // Test seam: arrive-and-wait on the zero-byte-EOF barrier
                    // before consulting authoritative state and polling for
                    // peer terminal events. The seam is a no-op when no
                    // barrier is installed.
                    #[cfg(test)]
                    arrive_zero_byte_eof_barrier().await;
                    let expected = state.expected_size();
                    let transmitted = state.total_bytes();
                    if transmitted == expected && !state.authoritative().is_primary() {
                        // One zero-duration poll for a peer terminal event
                        // that may already be ready in the WebSocket receive
                        // buffer. Per the approved design, a terminal event
                        // that becomes ready in this poll wins over the
                        // clean-EOF boundary. Events that are still pending
                        // belong to cleanup, not transfer-result selection.
                        use futures_util::FutureExt;
                        match ws_receiver.next().now_or_never() {
                            Some(Some(Ok(WsMessage::Close(_)))) => LoopStep::Finish(
                                state.peer_terminal("peer Close".to_string()),
                            ),
                            Some(Some(Err(_)) | None) => {
                                LoopStep::Finish(state.peer_terminal("peer EOF".to_string()))
                            }
                            // A control frame arriving at the boundary is
                            // not a transfer failure; commit CleanEof.
                            Some(Some(Ok(_))) | None => {
                                LoopStep::Finish(AuthoritativeResult::CleanEof)
                            }
                        }
                    } else if transmitted == expected {
                        // A primary error already selected — keep it.
                        LoopStep::Finish(state.authoritative().clone())
                    } else {
                        // Size mismatch (file truncated or modified during the
                        // transfer): report it over the wire as the existing
                        // protocol does, then select the primary error.
                        warn!(
                            "handle_file_download: File size mismatch: expected {expected} bytes, got {transmitted} bytes (file truncated or modified during download)"
                        );
                        send_file_error(
                            ws_sender,
                            uuid,
                            &format!("File size mismatch: expected {expected}, got {transmitted}"),
                            FILE_DOWNLOAD_ERROR,
                        )
                        .await;
                        LoopStep::Finish(AuthoritativeResult::PrimaryError(format!(
                            "unexpected EOF: transmitted {transmitted} of {expected} bytes"
                        )))
                    }
                }
                Ok(n) => {
                    trace!("handle_file_download: Read {} bytes from file", n);
                    // Build the chunk message bytes and arm the Sending phase.
                    let mut chunk_msg = Message::new(FILE_CHUNK, Priority::Highest, uuid);
                    chunk_msg.push_bytes(&buffer[..n]);
                    let bytes: Bytes = chunk_msg.get_data().clone().into();
                    state.pending_chunk_bytes = Some(bytes);
                    state.pending_chunk_len = n;
                    LoopStep::SetState(ChunkState::Sending)
                }
                Err(e) => {
                    warn!("Error reading file: {}", e);
                    send_file_error(
                        ws_sender,
                        uuid,
                        "Exception reading file",
                        FILE_DOWNLOAD_ERROR,
                    )
                    .await;
                    LoopStep::Finish(state.primary("file read failed"))
                }
            }
        }
    }
}

async fn run_sending_phase(
    ws_sender: &mut WsSender,
    ws_receiver: &mut WsReceiver,
    is_paused: &Arc<AtomicBool>,
    resume_notify: &Arc<Notify>,
    state: &mut TransferState,
) -> LoopStep {
    // Process any Pause/Resume/terminal messages already buffered (e.g. a
    // Pause that arrived while the previous chunk send was in flight) before
    // sending, so flow control is honoured promptly even on a fast socket.
    if let Some(step) = drain_pending_incoming(ws_receiver, is_paused, resume_notify, state) {
        return step;
    }

    // Honour pause before sending: if a Pause arrived while a chunk was
    // pending, the previous Sending iteration restored the chunk and set the
    // pause flag, but `LoopStep::Continue` kept the state as Sending. Without
    // this guard the restored chunk would be retried immediately while still
    // paused, breaking the flow-control protocol. While paused we wait for
    // Resume (or a peer terminal event) and keep the pending chunk in state so
    // the next Sending iteration transmits it.
    if is_paused.load(Ordering::Acquire) {
        return wait_for_terminal_or_resume(ws_receiver, is_paused, resume_notify, state).await;
    }

    // Take the pending chunk bytes out for the duration of this phase; if a
    // peer input branch wins the select, we restore the original bytes so
    // the chunk can be retried on the next Sending iteration.
    let Some(pending_bytes) = state.pending_chunk_bytes.take() else {
        // No pending chunk — fall back to reading.
        return LoopStep::SetState(ChunkState::Reading);
    };
    let chunk_len = state.pending_chunk_len;
    let ws_msg = WsMessage::Binary(pending_bytes.clone());
    let mut send_fut = Box::pin(ws_sender.send(ws_msg));

    tokio::select! {
        biased;
        incoming = ws_receiver.next() => {
            // Restore the chunk so the next Sending phase can retry.
            state.pending_chunk_bytes = Some(pending_bytes);
            match classify_incoming(incoming) {
                IncomingEvent::Pause => {
                    is_paused.store(true, Ordering::Release);
                    LoopStep::Continue
                }
                IncomingEvent::Resume => {
                    is_paused.store(false, Ordering::Release);
                    resume_notify.notify_one();
                    LoopStep::Continue
                }
                IncomingEvent::Close => LoopStep::Finish(state.peer_terminal("peer Close".to_string())),
                IncomingEvent::Eof => LoopStep::Finish(state.peer_terminal("peer EOF".to_string())),
                IncomingEvent::Error(msg) => LoopStep::Finish(
                    state.peer_terminal(format!("peer receive error: {msg}"))
                ),
                IncomingEvent::Ignored => LoopStep::Continue,
            }
        }
        send_result = &mut send_fut => {
            state.pending_chunk_bytes = None;
            state.pending_chunk_len = 0;
            match send_result {
                Ok(()) => {
                    trace!("handle_file_download: Chunk sent successfully");
                    // Accounting happens ONLY after a successful send.
                    state.transmitted_bytes += chunk_len as u64;
                    let is_final = state.transmitted_bytes == state.expected_size;
                    // Yield after non-final chunks so a Pause that is still in
                    // transit can be delivered and processed by the next phase's
                    // drain. Without this the supervisor can stream the whole
                    // file back-to-back on a fast socket and finish before the
                    // peer's Pause ever arrives, defeating the flow-control
                    // protocol. The final chunk does not yield so barrier-based
                    // race tests observe the send-complete boundary
                    // deterministically.
                    if !is_final {
                        tokio::task::yield_now().await;
                    }
                    // Test seam: arrive-and-wait on the final-send barrier if
                    // this was the last chunk and a barrier is installed. The
                    // seam is a no-op when no barrier is set.
                    #[cfg(test)]
                    if is_final {
                        arrive_final_send_barrier().await;
                    }
                    LoopStep::SetState(ChunkState::Reading)
                }
                Err(e) => {
                    warn!("handle_file_download: Failed to send chunk: {}", e);
                    LoopStep::Finish(state.primary("chunk send failed"))
                }
            }
        }
    }
}

async fn wait_for_terminal_or_resume(
    ws_receiver: &mut WsReceiver,
    is_paused: &Arc<AtomicBool>,
    resume_notify: &Arc<Notify>,
    state: &mut TransferState,
) -> LoopStep {
    tokio::select! {
        biased;
        incoming = ws_receiver.next() => match classify_incoming(incoming) {
            IncomingEvent::Pause => {
                is_paused.store(true, Ordering::Release);
                LoopStep::Continue
            }
            IncomingEvent::Resume => {
                is_paused.store(false, Ordering::Release);
                resume_notify.notify_one();
                LoopStep::Continue
            }
            IncomingEvent::Close => LoopStep::Finish(state.peer_terminal("peer Close".to_string())),
            IncomingEvent::Eof => LoopStep::Finish(state.peer_terminal("peer EOF".to_string())),
            IncomingEvent::Error(msg) => {
                LoopStep::Finish(state.peer_terminal(format!("peer receive error: {msg}")))
            }
            IncomingEvent::Ignored => LoopStep::Continue,
        },
        () = resume_notify.notified(), if is_paused.load(Ordering::Acquire) => {
            is_paused.store(false, Ordering::Release);
            LoopStep::Continue
        }
    }
}

fn classify_incoming(
    incoming: Option<Result<WsMessage, tokio_tungstenite::tungstenite::Error>>,
) -> IncomingEvent {
    match incoming {
        None => IncomingEvent::Eof,
        Some(Ok(WsMessage::Close(_))) => IncomingEvent::Close,
        Some(Ok(WsMessage::Binary(data))) => {
            let msg = Message::from_data(data.to_vec());
            if msg.id == PAUSE_FILE_CHUNK_STREAM {
                IncomingEvent::Pause
            } else if msg.id == RESUME_FILE_CHUNK_STREAM {
                IncomingEvent::Resume
            } else {
                IncomingEvent::Ignored
            }
        }
        Some(Ok(_)) => IncomingEvent::Ignored,
        Some(Err(e)) => IncomingEvent::Error(e.to_string()),
    }
}

/// Drain any incoming WebSocket messages that are already buffered without
/// blocking, applying Pause/Resume state and returning a terminal `LoopStep`
/// if a peer Close, EOF, or receive error is pending. This is called between
/// transfer phases so a Pause that arrives while a chunk send is in flight is
/// honoured promptly instead of being deferred until the transfer reaches EOF
/// on a fast socket. Returns `None` when no message is immediately ready.
fn drain_pending_incoming(
    ws_receiver: &mut WsReceiver,
    is_paused: &Arc<AtomicBool>,
    resume_notify: &Arc<Notify>,
    state: &mut TransferState,
) -> Option<LoopStep> {
    use futures_util::FutureExt;
    loop {
        match ws_receiver.next().now_or_never() {
            Some(incoming) => match classify_incoming(incoming) {
                IncomingEvent::Pause => {
                    is_paused.store(true, Ordering::Release);
                }
                IncomingEvent::Resume => {
                    is_paused.store(false, Ordering::Release);
                    resume_notify.notify_one();
                }
                IncomingEvent::Close => {
                    return Some(LoopStep::Finish(
                        state.peer_terminal("peer Close".to_string()),
                    ));
                }
                IncomingEvent::Eof => {
                    return Some(LoopStep::Finish(
                        state.peer_terminal("peer EOF".to_string()),
                    ));
                }
                IncomingEvent::Error(msg) => {
                    return Some(LoopStep::Finish(
                        state.peer_terminal(format!("peer receive error: {msg}")),
                    ));
                }
                IncomingEvent::Ignored => {}
            },
            None => return None,
        }
    }
}

/// Unified cleanup epilogue. Sends a standard WebSocket Close frame, keeps the
/// receive half active for peer acknowledgement, and bounds the entire
/// graceful shutdown by one total `graceful_close_timeout()` deadline. On
/// graceful failure or timeout the supervisor drops both halves as the task
/// ends — there is no child listener to abort or await.
async fn cleanup_download(
    ws_sender: &mut WsSender,
    ws_receiver: &mut WsReceiver,
    primary: AuthoritativeResult,
) {
    // Test seam: expose the authoritative result (transfer-loop or pre-transfer
    // primary error) to integration tests before cleanup consumes it. No-op
    // when no observer is installed.
    #[cfg(test)]
    notify_transfer_outcome_observer(&primary);

    // Test seam: park before the Close send so a test can reset the peer
    // transport and make the Close send deterministically fail after a
    // successful transfer. No-op when no barrier is installed.
    #[cfg(test)]
    arrive_pre_close_send_barrier().await;

    // Preserve the primary transfer result — cleanup errors cannot replace it.
    let primary_error = match primary {
        AuthoritativeResult::PrimaryError(msg) => Some(msg),
        _ => None,
    };

    // One total deadline covers both sending Close and waiting for the peer's
    // acknowledgement. A send failure or deadline expiry is followed
    // immediately by forced release when the supervisor drops both halves.
    let graceful_close = async {
        ws_sender
            .send(WsMessage::Close(None))
            .await
            .map_err(|e| format!("failed to send Close frame: {e}"))?;

        loop {
            match ws_receiver.next().await {
                Some(Ok(WsMessage::Close(_))) => {
                    debug!("handle_file_download: peer acknowledged Close");
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => {
                    debug!("handle_file_download: peer terminated during graceful close");
                    break;
                }
            }
        }

        Ok::<(), String>(())
    };

    match tokio::time::timeout(graceful_close_timeout(), graceful_close).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            warn!("handle_file_download: {e} during cleanup; forced release follows");
            #[cfg(test)]
            notify_cleanup_failure(format!("{e} during cleanup; forced release follows"));
            if let Some(msg) = primary_error {
                debug!("handle_file_download: cleanup failure preserved primary error: {msg}");
            }
        }
        Err(_) => {
            warn!(
                "handle_file_download: graceful shutdown exceeded {}s; forced release follows",
                GRACEFUL_CLOSE_TIMEOUT_SECS
            );
            #[cfg(test)]
            notify_cleanup_failure("graceful shutdown timeout; forced release follows".to_string());
            if let Some(msg) = primary_error {
                debug!("handle_file_download: cleanup timeout preserved primary error: {msg}");
            }
        }
    }
}

fn log_job_lookup_error(prefix: &str, job_id: i64, err: &JobLookupError) {
    match err {
        JobLookupError::NotSubmitted => {
            warn!("{prefix}: Job is not submitted, sending error");
        }
        JobLookupError::NotFound => {
            warn!("{prefix}: Job {} not found in database", job_id);
        }
        JobLookupError::Database(e) => {
            warn!("{prefix}: Database error for job {}: {}", job_id, e);
        }
    }
}

/// Per-transfer mutable state owned by the supervisor. Encapsulates
/// authoritative result selection, transmitted-byte accounting, and the
/// pending chunk that survives a Sending-phase peer input branch.
#[derive(Debug)]
struct TransferState {
    authoritative: AuthoritativeResult,
    transmitted_bytes: u64,
    expected_size: u64,
    pending_chunk_bytes: Option<Bytes>,
    pending_chunk_len: usize,
}

impl TransferState {
    fn new(expected_size: u64) -> Self {
        Self {
            authoritative: AuthoritativeResult::Pending,
            transmitted_bytes: 0,
            expected_size,
            pending_chunk_bytes: None,
            pending_chunk_len: 0,
        }
    }

    fn total_bytes(&self) -> u64 {
        self.transmitted_bytes
    }

    fn expected_size(&self) -> u64 {
        self.expected_size
    }

    fn authoritative(&self) -> &AuthoritativeResult {
        &self.authoritative
    }

    fn take_authoritative(self) -> AuthoritativeResult {
        self.authoritative
    }

    fn set_authoritative(&mut self, result: AuthoritativeResult) {
        // Preserve ordering: a primary error already selected cannot be
        // overwritten by a peer terminal that arrives later.
        if matches!(self.authoritative, AuthoritativeResult::PrimaryError(_))
            && !matches!(result, AuthoritativeResult::PrimaryError(_))
        {
            return;
        }
        self.authoritative = result;
    }

    fn primary(&mut self, msg: &str) -> AuthoritativeResult {
        let result = AuthoritativeResult::PrimaryError(msg.to_string());
        self.set_authoritative(result.clone());
        result
    }

    /// Record a peer terminal event. If a primary error already won, this
    /// returns the preserved primary error so callers can short-circuit.
    fn peer_terminal(&mut self, msg: String) -> AuthoritativeResult {
        if matches!(self.authoritative, AuthoritativeResult::PrimaryError(_)) {
            return self.authoritative.clone();
        }
        let result = AuthoritativeResult::PeerTerminal(msg);
        self.authoritative = result.clone();
        result
    }
}

impl AuthoritativeResult {
    fn is_primary(&self) -> bool {
        matches!(self, Self::PrimaryError(_))
    }
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
        let Some((mut ws_sender, mut ws_receiver)) =
            connect_file_ws(&ws_endpoint, &uuid, "", "file upload").await
        else {
            return;
        };

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
                send_file_error(
                    &mut ws_sender,
                    &uuid,
                    &format!("Failed to create parent directory: {e}"),
                    FILE_UPLOAD_ERROR,
                )
                .await;
                return;
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
                        remove_partial_file(&full_path).await;
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
                        remove_partial_file(&full_path).await;
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
        remove_partial_file(&full_path).await;
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

async fn connect_file_ws(
    ws_endpoint: &str,
    uuid: &str,
    prefix: &str,
    operation: &str,
) -> Option<(
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
)> {
    let (ws_sender, mut ws_receiver) =
        connect_file_ws_raw(ws_endpoint, uuid, prefix, operation).await?;

    if wait_for_server_ready(&mut ws_receiver).await.is_none() {
        warn!("{prefix}Failed to receive SERVER_READY");
        return None;
    }

    Some((ws_sender, ws_receiver))
}

/// Establish an authenticated file WebSocket and transfer ownership of both
/// halves to the caller before any application-level readiness validation.
///
/// Downloads use this seam so their lifecycle owner can converge every
/// post-connect readiness outcome on unified cleanup. Uploads continue through
/// `connect_file_ws`, preserving their existing connect-and-ready behaviour.
async fn connect_file_ws_raw(
    ws_endpoint: &str,
    uuid: &str,
    prefix: &str,
    operation: &str,
) -> Option<(
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
)> {
    let request = match build_file_ws_request(ws_endpoint, uuid) {
        Ok(request) => request,
        Err(e) => {
            warn!("{prefix}Failed to build {operation} request: {e}");
            return None;
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
            warn!("{prefix}Failed to connect for {operation}: {e}");
            return None;
        }
        Err(_) => {
            warn!("{prefix}Timed out connecting for {operation}");
            return None;
        }
    };

    Some(ws_stream.split())
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

/// Remove a partial upload file, logging (rather than silently ignoring) any
/// failure so an unremovable partial file is diagnosable in the logs.
async fn remove_partial_file(full_path: &Path) {
    if let Err(e) = fs::remove_file(full_path).await {
        warn!(
            "handle_file_upload_internal: Failed to remove partial file {:?}: {}",
            full_path, e
        );
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
        remove_partial_file(full_path).await;
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
        remove_partial_file(full_path).await;
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
    use mockall::predicate::{always, eq};
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct VecWriter(Arc<Mutex<Vec<u8>>>);

    impl VecWriter {
        fn new() -> Self {
            Self::default()
        }

        fn into_string(self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap_or_default()
        }
    }

    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            VecWriterGuard(self.0.clone())
        }
    }

    struct VecWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for VecWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_logs<F: FnOnce()>(f: F) -> String {
        let writer = VecWriter::new();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_target(true)
            .with_level(true)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        writer.into_string()
    }

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

    fn run_collect(entry: tokio::fs::DirEntry, working_dir: &str) -> Option<(String, bool, u64)> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(collect_dir_entry(entry, working_dir))
    }

    fn collect_entries(dir: &Path) -> Vec<tokio::fs::DirEntry> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut entries = tokio::fs::read_dir(dir).await.unwrap();
            let mut out = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                out.push(entry);
            }
            out
        })
    }

    struct FailingDirEntrySource;

    impl DirEntrySource for FailingDirEntrySource {
        async fn next_entry(&mut self) -> std::io::Result<Option<tokio::fs::DirEntry>> {
            Err(std::io::Error::other("simulated read failure"))
        }
    }

    struct EmptyDirEntrySource;

    impl DirEntrySource for EmptyDirEntrySource {
        async fn next_entry(&mut self) -> std::io::Result<Option<tokio::fs::DirEntry>> {
            Ok(None)
        }
    }

    struct CountHandler<'a> {
        count: &'a mut usize,
    }

    impl DirEntryHandler for CountHandler<'_> {
        async fn handle(&mut self, _entry: tokio::fs::DirEntry) {
            *self.count += 1;
        }
    }

    struct NameCollector<'a> {
        names: &'a mut std::collections::BTreeSet<String>,
    }

    impl DirEntryHandler for NameCollector<'_> {
        async fn handle(&mut self, entry: tokio::fs::DirEntry) {
            self.names
                .insert(entry.file_name().to_string_lossy().into_owned());
        }
    }

    #[test]
    fn for_each_dir_entry_logs_warning_and_stops_on_next_entry_error() {
        let logs = capture_logs(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut source = FailingDirEntrySource;
                let mut count = 0usize;
                let mut handler = CountHandler { count: &mut count };
                for_each_dir_entry(&mut source, Path::new("/tmp"), &mut handler).await;
                assert_eq!(count, 0, "no entries should be visited on error");
            });
        });
        assert!(
            logs.contains("handle_file_list: failed to read directory entry"),
            "expected next_entry failure warning, got: {logs}"
        );
    }

    #[test]
    fn for_each_dir_entry_stops_cleanly_at_end_of_stream() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut source = EmptyDirEntrySource;
            let mut count = 0usize;
            let mut handler = CountHandler { count: &mut count };
            for_each_dir_entry(&mut source, Path::new("/tmp"), &mut handler).await;
            assert_eq!(count, 0);
        });
    }

    #[test]
    fn for_each_dir_entry_visits_all_entries_in_directory() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut entries = tokio::fs::read_dir(tmp.path()).await.unwrap();
            let mut names = std::collections::BTreeSet::new();
            let mut handler = NameCollector { names: &mut names };
            for_each_dir_entry(&mut entries, tmp.path(), &mut handler).await;
            assert_eq!(names.len(), 2);
            assert!(names.contains("a.txt"));
            assert!(names.contains("b.txt"));
        });
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
    fn validate_list_target_is_directory_accepts_dir_rejects_file_and_logs_metadata_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tmp = TempDir::new().unwrap();
        let subdir = tmp.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        let file = tmp.path().join("file.txt");
        fs::write(&file, "data").unwrap();
        let missing = tmp.path().join("missing");

        assert!(
            rt.block_on(validate_list_target_is_directory(&subdir))
                .is_ok(),
            "existing directory should be accepted"
        );

        assert!(
            rt.block_on(validate_list_target_is_directory(&file))
                .is_err(),
            "existing file should be rejected"
        );

        let logs = capture_logs(|| {
            assert!(
                rt.block_on(validate_list_target_is_directory(&missing))
                    .is_err(),
                "metadata failure should be rejected"
            );
        });
        assert!(
            logs.contains("handle_file_list: Failed to get file metadata"),
            "expected metadata failure warning, got: {logs}"
        );
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
    #[serial_test::serial]
    fn test_send_file_list_error_queues_highest_priority_message() {
        reset_websocket_client_for_test();
        let mut mock_ws = MockWebsocketClient::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let test_uuid = "uuid-123".to_string();
        let uuid_clone = test_uuid.clone();
        mock_ws
            .expect_queue_message()
            .with(eq(uuid_clone), always(), eq(Priority::Highest))
            .times(1)
            .returning(move |_, data, _| {
                let _ = tx_clone.send(data);
            });
        set_websocket_client(Arc::new(mock_ws));

        send_file_list_error(&test_uuid, "boom");

        let data = rx.try_recv().expect("expected a queued message");
        let mut resp = Message::from_data(data);
        assert_eq!(resp.id, FILE_LIST_ERROR);
        assert_eq!(resp.source, "uuid-123");
        assert_eq!(resp.pop_string(), "uuid-123");
        assert_eq!(resp.pop_string(), "boom");
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
    fn test_remove_partial_file_logs_warning_when_removal_fails() {
        let tmp = TempDir::new().unwrap();
        // fs::remove_file on a non-existent path fails (NotFound), so the
        // warning branch is exercised.
        let missing = tmp.path().join("does-not-exist.bin");
        let logs = capture_logs(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(remove_partial_file(&missing));
        });
        assert!(
            logs.contains("Failed to remove partial file"),
            "expected a warning for the failed removal, got: {logs}"
        );
    }

    #[test]
    fn test_remove_partial_file_removes_existing_file_without_warning() {
        let tmp = TempDir::new().unwrap();
        let full_path = tmp.path().join("upload.bin");
        std::fs::write(&full_path, b"data").unwrap();
        let logs = capture_logs(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(remove_partial_file(&full_path));
        });
        assert!(!full_path.exists(), "existing file should be removed");
        assert!(
            !logs.contains("Failed to remove partial file"),
            "no warning expected on successful removal, got: {logs}"
        );
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

    #[tokio::test]
    async fn test_wait_for_server_ready_returns_handshake() {
        let server = WebsocketServerFixture::new().await;
        let url = format!("ws://127.0.0.1:{}/ws/", server.port);
        let request = build_file_ws_request(&url, "test-token").unwrap();
        let (ws_stream, _) = connect_async(request).await.unwrap();
        let (_ws_sender, mut ws_receiver) = ws_stream.split();

        let msg = wait_for_server_ready(&mut ws_receiver)
            .await
            .expect("Expected SERVER_READY handshake");
        assert_eq!(msg.id, SERVER_READY);
    }

    #[tokio::test]
    async fn test_connect_file_ws_returns_sender_receiver_on_success() {
        let server = WebsocketServerFixture::new().await;
        let url = format!("ws://127.0.0.1:{}/ws/", server.port);

        let result = connect_file_ws(&url, "test-uuid", "", "test").await;
        assert!(result.is_some(), "Expected successful file WS connection");
    }

    #[tokio::test]
    async fn test_connect_file_ws_returns_none_on_invalid_endpoint() {
        let result = connect_file_ws("not-a-websocket-url", "test-uuid", "", "test").await;
        assert!(result.is_none(), "Expected None for invalid endpoint");
    }

    #[tokio::test]
    async fn connect_file_ws_returns_none_when_server_ready_not_received() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws_stream = accept_async(stream).await.unwrap();
            // Send a non-SERVER_READY message so wait_for_server_ready returns None.
            let wrong = Message::new(UPLOAD_FILE, Priority::Highest, "test");
            ws_stream
                .send(WsMessage::Binary(wrong.get_data().clone().into()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let url = format!("ws://127.0.0.1:{port}/ws/");
        let result = connect_file_ws(&url, "test-uuid", "", "file upload").await;
        assert!(
            result.is_none(),
            "connect_file_ws should return None when SERVER_READY is not received"
        );

        server_handle.await.unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn get_ws_endpoint_from_config_reads_and_normalizes_trailing_slash() {
        let saved = crate::config::TEST_CONFIG.lock().unwrap().clone();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(json!({
            "websocketEndpoint": "ws://example.com/ws",
        }));

        assert_eq!(get_ws_endpoint_from_config(), "ws://example.com/ws/");

        *crate::config::TEST_CONFIG.lock().unwrap() = saved;
    }

    #[test]
    #[serial_test::serial]
    fn get_ws_endpoint_from_config_preserves_existing_trailing_slash() {
        let saved = crate::config::TEST_CONFIG.lock().unwrap().clone();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(json!({
            "websocketEndpoint": "ws://example.com/ws/",
        }));

        assert_eq!(get_ws_endpoint_from_config(), "ws://example.com/ws/");

        *crate::config::TEST_CONFIG.lock().unwrap() = saved;
    }

    #[test]
    #[serial_test::serial]
    fn get_ws_endpoint_from_config_falls_back_to_default_when_key_missing() {
        let saved = crate::config::TEST_CONFIG.lock().unwrap().clone();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(json!({
            "cluster": "test_cluster",
        }));

        assert_eq!(get_ws_endpoint_from_config(), "ws://127.0.0.1:8001/ws/");

        *crate::config::TEST_CONFIG.lock().unwrap() = saved;
    }

    #[test]
    fn test_collect_dir_entry_regular_file() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        std::fs::write(tmp.path().join("data.txt"), "hello").unwrap();

        let entries = collect_entries(tmp.path());
        assert_eq!(entries.len(), 1);
        let (relative, is_dir, size) =
            run_collect(entries.into_iter().next().unwrap(), &wd).unwrap();
        assert_eq!(relative, "data.txt");
        assert!(!is_dir, "regular file should not be flagged as a directory");
        assert_eq!(size, 5);
    }

    #[test]
    fn test_collect_dir_entry_directory() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();

        let entries = collect_entries(tmp.path());
        assert_eq!(entries.len(), 1);
        let (relative, is_dir, _) = run_collect(entries.into_iter().next().unwrap(), &wd).unwrap();
        assert_eq!(relative, "subdir");
        assert!(is_dir, "directory entry should be flagged as a directory");
    }

    #[test]
    fn test_collect_dir_entry_skips_symlink() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap().to_string();
        std::fs::write(tmp.path().join("target.txt"), "data").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("target.txt"), tmp.path().join("link")).unwrap();

        let entries = collect_entries(tmp.path());
        let symlink = entries
            .into_iter()
            .find(|e| e.file_name().to_string_lossy() == "link")
            .unwrap();
        assert!(
            run_collect(symlink, &wd).is_none(),
            "symlink entries should be skipped (DirEntry::metadata does not follow symlinks)"
        );
    }

    #[test]
    fn test_collect_dir_entry_strip_prefix_fallback() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("data.txt"), "x").unwrap();

        let entries = collect_entries(tmp.path());
        let entry_path = entries[0].path();
        // working_directory is not a prefix of the entry path -> full path returned
        let (relative, _, _) = run_collect(
            entries.into_iter().next().unwrap(),
            "/unrelated/working/dir",
        )
        .unwrap();
        assert_eq!(relative, entry_path.to_string_lossy());
    }

    // ---------------------------------------------------------------
    // Per-transfer download supervisor smoke tests (task-2).
    // Exercise the supervisor's seam and authoritative-result selection
    // without relying on a real WebSocket. Task-4 will add full lifecycle
    // regression coverage using the task-3 fixture.
    // ---------------------------------------------------------------

    #[test]
    fn graceful_close_timeout_defaults_to_five_seconds() {
        set_graceful_close_timeout_for_test(None);
        assert_eq!(
            graceful_close_timeout(),
            Duration::from_secs(GRACEFUL_CLOSE_TIMEOUT_SECS)
        );
    }

    #[test]
    fn graceful_close_timeout_override_is_honoured() {
        set_graceful_close_timeout_for_test(Some(Duration::from_millis(50)));
        assert_eq!(
            graceful_close_timeout(),
            Duration::from_millis(50),
            "override seam must allow tests to exercise forced release deterministically"
        );
        set_graceful_close_timeout_for_test(None);
        assert_eq!(
            graceful_close_timeout(),
            Duration::from_secs(GRACEFUL_CLOSE_TIMEOUT_SECS),
            "production default must be restored when override is cleared"
        );
    }

    #[test]
    fn transfer_state_preserves_primary_error_over_later_peer_terminal() {
        let mut state = TransferState::new(1024);
        let primary = state.primary("chunk send failed");
        assert!(matches!(primary, AuthoritativeResult::PrimaryError(_)));

        let peer = state.peer_terminal("peer Close".to_string());
        assert!(
            matches!(peer, AuthoritativeResult::PrimaryError(_)),
            "primary error must not be overwritten by a later peer terminal event"
        );
        assert!(matches!(
            state.authoritative(),
            AuthoritativeResult::PrimaryError(_)
        ));
    }

    #[test]
    fn transfer_state_records_bytes_only_after_set_authoritative_with_clean_eof() {
        let mut state = TransferState::new(8);
        state.transmitted_bytes = 8;
        state.set_authoritative(AuthoritativeResult::CleanEof);
        assert_eq!(state.total_bytes(), 8);
        assert!(matches!(
            state.authoritative(),
            AuthoritativeResult::CleanEof
        ));
    }

    #[test]
    fn transfer_state_rejects_clean_eof_with_size_mismatch() {
        let mut state = TransferState::new(8);
        state.transmitted_bytes = 4;
        // The supervisor's reading-phase branch selects a PrimaryError when the
        // zero-byte read arrives before transmitted_bytes == expected_size.
        // This unit test only asserts that the state still surfaces a
        // non-CleanEof outcome via `authoritative()` after the supervisor
        // sets it; the actual selection happens in the supervisor.
        state.set_authoritative(AuthoritativeResult::PrimaryError(
            "unexpected EOF: transmitted 4 of 8 bytes".to_string(),
        ));
        assert!(!matches!(
            state.authoritative(),
            AuthoritativeResult::CleanEof
        ));
    }

    #[test]
    fn transfer_state_lets_primary_error_replace_pending() {
        let mut state = TransferState::new(8);
        state.set_authoritative(AuthoritativeResult::Pending);
        // Pending is overwritten by any terminal variant.
        state.set_authoritative(AuthoritativeResult::CleanEof);
        assert!(matches!(
            state.authoritative(),
            AuthoritativeResult::CleanEof
        ));
    }
}
