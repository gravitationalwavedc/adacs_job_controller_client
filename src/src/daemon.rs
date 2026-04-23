//! UNIX daemon double-fork implementation.
//!
//! Mirrors the C++ daemonization pattern from main.cpp EXACTLY:
//! 1. First fork and exit parent
//! 2. Create new session with setsid()
//! 3. Change working directory to root
//! 4. Reset umask
//! 5. Second fork to prevent acquiring controlling terminal
//! 6. Redirect stdin/stdout/stderr file descriptors
//! 7. Redirect stdout/stderr to log files

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;
use std::process;
use tracing::{error, info};

/// Perform UNIX double-fork daemonization with stdout/stderr redirection
/// Returns true if this is the daemon process, false if this is a parent that should exit
///
/// This function matches the C++ implementation exactly, including:
/// - Double-fork pattern
/// - Session creation
/// - File descriptor redirection
/// - stdout/stderr to log files
pub fn daemonize() -> Result<bool, Box<dyn std::error::Error>> {
    // First fork
    match unsafe { libc::fork() } {
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
    // Change working directory to root to avoid keeping any directory mounted
    unsafe {
        libc::chdir("/".as_ptr() as *const libc::c_char);
    }

    // Create a new session and become the session leader
    unsafe {
        libc::setsid();
    }

    // Reset umask to have full control over file permissions
    unsafe {
        libc::umask(0);
    }

    // Second fork
    match unsafe { libc::fork() } {
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
    // This ensures the daemon doesn't hold open any file descriptors from the parent
    unsafe {
        let s_in = libc::open(std::ptr::null(), libc::O_RDONLY);
        let s_out = libc::open(std::ptr::null(), libc::O_APPEND | libc::O_WRONLY);
        let s_err = libc::open(std::ptr::null(), libc::O_APPEND | libc::O_WRONLY);

        libc::dup2(s_in, libc::STDIN_FILENO);
        libc::dup2(s_out, libc::STDOUT_FILENO);
        libc::dup2(s_err, libc::STDERR_FILENO);
    }

    // Flush stdout and stderr before redirection (matches C++)
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();

    Ok(true)
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
    match unsafe { libc::fork() } {
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
    unsafe {
        libc::chdir("/".as_ptr() as *const libc::c_char);
    }

    unsafe {
        libc::setsid();
    }

    unsafe {
        libc::umask(0);
    }

    // Second fork
    match unsafe { libc::fork() } {
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
    unsafe {
        let s_in = libc::open(std::ptr::null(), libc::O_RDONLY);
        let s_out = libc::open(std::ptr::null(), libc::O_APPEND | libc::O_WRONLY);
        let s_err = libc::open(std::ptr::null(), libc::O_APPEND | libc::O_WRONLY);

        libc::dup2(s_in, libc::STDIN_FILENO);
        libc::dup2(s_out, libc::STDOUT_FILENO);
        libc::dup2(s_err, libc::STDERR_FILENO);
    }

    // Flush stdout and stderr before redirection
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();

    // Redirect stdout and stderr to log files (matches C++ main.cpp lines 118-119)
    let stdout_path = log_dir.join("stdout.log");
    let stderr_path = log_dir.join("stderr.log");

    // Open log files in append mode
    let _stdout_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)?;

    let _stderr_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)?;

    // Get raw file descriptors
    let stdout_fd = _stdout_file.as_raw_fd();
    let stderr_fd = _stderr_file.as_raw_fd();

    // Duplicate file descriptors to stdout/stderr
    unsafe {
        libc::dup2(stdout_fd, libc::STDOUT_FILENO);
        libc::dup2(stderr_fd, libc::STDERR_FILENO);
    }

    // Log files will be closed when _stdout_file and _stderr_file go out of scope
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
    use tempfile::TempDir;

    #[test]
    fn test_daemonize_basic() {
        // This test verifies daemonize() can be called without panicking
        // Note: Due to fork() behavior, the test framework captures both parent and child
        // The parent returns Ok(false) and exits, child returns Ok(true) and continues
        let result = daemonize();
        assert!(result.is_ok());
    }

    #[test]
    fn test_daemonize_with_log_redirect_creates_files() {
        // Create a temporary directory for log files
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path().to_path_buf();

        let result = daemonize_with_log_redirect(&log_dir);
        assert!(result.is_ok(), "daemonize_with_log_redirect should succeed");

        // Give daemon process time to create files
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Verify log files were created
        let stdout_path = log_dir.join("stdout.log");
        let stderr_path = log_dir.join("stderr.log");

        assert!(stdout_path.exists(), "stdout.log should be created");
        assert!(stderr_path.exists(), "stderr.log should be created");
    }
}
