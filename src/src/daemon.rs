//! UNIX daemon double-fork implementation.
//!
//! Mirrors the C++ daemonization pattern from main.cpp EXACTLY:
//! 1. First fork and exit parent
//! 2. Create new session with `setsid()`
//! 3. Change working directory to root
//! 4. Reset umask
//! 5. Second fork to prevent acquiring controlling terminal
//! 6. Redirect stdin/stdout/stderr file descriptors
//! 7. Redirect stdout/stderr to log files

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::process;
use tracing::{error, info, warn};

#[cfg(test)]
use parking_lot::Mutex;

/// `fork` wrapper that honours the test-only override.
fn fork_wrapper() -> libc::pid_t {
    #[cfg(test)]
    if let Some(f) = FORK_OVERRIDE.lock().as_ref() {
        return f();
    }
    // SAFETY: libc::fork() is a raw syscall; returns -1 on error, 0 in child, PID in parent.
    unsafe { libc::fork() }
}

// ─── Test-only fork override seam ───────────────────────────────────────────
// `daemonize_with_log_redirect`'s two fork-failure branches (`fork #1 failed`
// and `fork #2 failed`) are unreachable through normal operation. This seam lets
// tests force `fork()` to return `-1`. Tests run serially (`--test-threads=1`),
// so the global override cannot race across tests.

#[cfg(test)]
type ForkFn = Box<dyn Fn() -> libc::pid_t + Send>;

#[cfg(test)]
static FORK_OVERRIDE: Mutex<Option<ForkFn>> = Mutex::new(None);

/// Test-only: install an override for `fork`, returning the previously-installed
/// override (if any). Pass `None` to clear it.
#[cfg(test)]
pub fn set_fork_override(f: Option<ForkFn>) -> Option<ForkFn> {
    let mut guard = FORK_OVERRIDE.lock();
    std::mem::replace(&mut *guard, f)
}

/// Perform UNIX double-fork daemonization with stdout/stderr redirection to log files
///
/// This is the full daemonization that matches C++ exactly, including:
/// - Double-fork pattern
/// - Session creation
/// - File descriptor redirection
/// - stdout/stderr to specific log files
///
/// # Arguments
/// * `log_dir` - Directory where stdout.log and stderr.log will be written
///
/// # Returns
/// * `Ok(true)` - This is the daemon process
/// * `Ok(false)` - This is a parent process that should exit
/// * `Err(e)` - Daemonization failed
pub fn daemonize_with_log_redirect(
    log_dir: &std::path::Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    // First fork
    match fork_wrapper() {
        -1 => {
            error!("fork #1 failed");
            return Err("fork #1 failed".into());
        }
        pid if pid > 0 => {
            // First parent - exit immediately
            info!("First parent exiting (pid: {})", pid);
            return Ok(false);
        }
        0 => {
            // First child - continue to daemonize
        }
        _ => unreachable!(),
    }

    // Decouple from parent environment
    // SAFETY: c"/" is a valid null-terminated C string pointing to a valid directory.
    let ret = unsafe { libc::chdir(c"/".as_ptr()) };
    if ret != 0 {
        tracing::warn!("chdir to / failed: {}", std::io::Error::last_os_error());
    }

    // Create a new session and become the session leader
    // SAFETY: setsid() has no undefined behavior; returns session ID or -1 on error.
    let ret = unsafe { libc::setsid() };
    if ret == -1 {
        warn!("setsid() failed: {}", std::io::Error::last_os_error());
    }

    // Set a restrictive umask so created files (e.g. log files) are not
    // world-writable. 0o022 masks group/other write, yielding 0o644 for
    // files created with the default 0o666 mode.
    // SAFETY: umask() always succeeds and returns the previous mask value.
    unsafe {
        libc::umask(0o022);
    }

    // Second fork
    match fork_wrapper() {
        -1 => {
            error!("fork #2 failed");
            return Err("fork #2 failed".into());
        }
        pid if pid > 0 => {
            // Second parent - exit
            info!("Second parent exiting (pid: {})", pid);
            return Ok(false);
        }
        0 => {
            // Second child - this is the daemon process
            info!("Daemon process started (pid: {})", process::id());
        }
        _ => unreachable!(),
    }

    // We are now the daemon process

    // Redirect standard file descriptors (matches C++ main.cpp lines 112-120)
    // Open /dev/null using safe Rust, then dup2 to stdio slots.
    // into_raw_fd() transfers ownership so we manually close below,
    // avoiding a double-close if the fd happened to land on 0/1/2.
    let fd_in = std::fs::File::open("/dev/null")
        .map_err(|e| format!("open /dev/null: {e}"))?
        .into_raw_fd();
    let fd_out = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .map_err(|e| format!("open /dev/null: {e}"))?
        .into_raw_fd();
    // Flush stdout and stderr before redirection
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();

    // SAFETY: fd_in and fd_out are valid open FDs from into_raw_fd(); dup2 returns -1 on error.
    unsafe {
        if libc::dup2(fd_in, libc::STDIN_FILENO) == -1 {
            warn!("dup2(stdin) failed: {}", std::io::Error::last_os_error());
        }
        if libc::dup2(fd_out, libc::STDOUT_FILENO) == -1 {
            warn!("dup2(stdout) failed: {}", std::io::Error::last_os_error());
        }
        if libc::dup2(fd_out, libc::STDERR_FILENO) == -1 {
            warn!("dup2(stderr) failed: {}", std::io::Error::last_os_error());
        }
        // Close the /dev/null source fds unless dup2 already mapped them onto stdio.
        if fd_in > 2 {
            libc::close(fd_in);
        }
        if fd_out > 2 {
            libc::close(fd_out);
        }
    }

    // Redirect stdout and stderr to log files (matches C++ main.cpp lines 118-119)
    let stdout_path = log_dir.join("stdout.log");
    let stderr_path = log_dir.join("stderr.log");

    // Open log files in append mode
    let stdout_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)?;

    let stderr_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)?;

    // Get raw file descriptors
    let stdout_fd = stdout_file.as_raw_fd();
    let stderr_fd = stderr_file.as_raw_fd();

    // Duplicate file descriptors to stdout/stderr
    // SAFETY: stdout_fd and stderr_fd are valid open FDs from as_raw_fd(); dup2 returns -1 on error.
    unsafe {
        if libc::dup2(stdout_fd, libc::STDOUT_FILENO) == -1 {
            warn!("dup2(stdout) failed: {}", std::io::Error::last_os_error());
        }
        if libc::dup2(stderr_fd, libc::STDERR_FILENO) == -1 {
            warn!("dup2(stderr) failed: {}", std::io::Error::last_os_error());
        }
    }

    // Log files will be closed when stdout_file and stderr_file go out of scope
    // but the duplicated file descriptors will remain open

    info!(
        "Daemon stdout/stderr redirected to {:?} and {:?}",
        stdout_path, stderr_path
    );

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;
    use test_fork::test;

    /// RAII guard that installs a `fork` override for the duration of a test and
    /// restores the previous override on drop.
    struct ForkOverrideGuard {
        prev: Option<ForkFn>,
    }

    impl ForkOverrideGuard {
        fn install(f: Option<ForkFn>) -> Self {
            let prev = set_fork_override(f);
            Self { prev }
        }
    }

    impl Drop for ForkOverrideGuard {
        fn drop(&mut self) {
            set_fork_override(self.prev.take());
        }
    }

    #[test]
    #[serial]
    fn test_daemonize_with_log_redirect_fork1_failure() {
        let _guard = ForkOverrideGuard::install(Some(Box::new(|| -1)));
        let result = daemonize_with_log_redirect(std::path::Path::new("/tmp"));
        assert_eq!(result.unwrap_err().to_string(), "fork #1 failed");
    }

    #[test]
    #[serial]
    fn test_daemonize_with_log_redirect_fork2_failure() {
        // First fork returns 0 (first child continues), second fork returns -1.
        let calls = Arc::new(AtomicI32::new(0));
        let calls2 = Arc::clone(&calls);
        let _guard = ForkOverrideGuard::install(Some(Box::new(move || {
            if calls2.fetch_add(1, Ordering::SeqCst) == 0 {
                0
            } else {
                -1
            }
        })));

        // The first child path calls chdir("/") and umask(); save and restore
        // them so this non-forking test does not perturb the shared process.
        let orig_dir = std::env::current_dir().unwrap();
        let orig_umask = unsafe { libc::umask(0) };
        unsafe { libc::umask(orig_umask) };

        let result = daemonize_with_log_redirect(std::path::Path::new("/tmp"));
        assert_eq!(result.unwrap_err().to_string(), "fork #2 failed");

        std::env::set_current_dir(&orig_dir).unwrap();
        unsafe { libc::umask(orig_umask) };
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    #[serial]
    fn test_daemonize_with_log_redirect_creates_files() {
        // Create a temporary directory for log files
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path().to_path_buf();
        std::mem::forget(temp_dir); // Prevent deletion of the directory during fork lifetime

        let result = daemonize_with_log_redirect(&log_dir);
        assert!(result.is_ok(), "daemonize_with_log_redirect should succeed");

        // Give daemon process time to create files (retry loop to avoid test flakiness under CPU load)
        let stdout_path = log_dir.join("stdout.log");
        let stderr_path = log_dir.join("stderr.log");
        let mut created = false;
        for _ in 0..200 {
            if stdout_path.exists() && stderr_path.exists() {
                created = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            created,
            "stdout.log and stderr.log should be created by daemon"
        );

        // Log files should not be world-writable (restrictive umask applied)
        for path in [&stdout_path, &stderr_path] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o002,
                0,
                "{} should not be world-writable (mode {mode:o})",
                path.display()
            );
        }
    }
}
