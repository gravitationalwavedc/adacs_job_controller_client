mod bundle_db;
mod bundle_interface;
mod bundle_logging;
mod bundle_manager;
mod config;
mod daemon;
mod db;
mod files;
mod jobs;
mod logging;
mod messaging;
mod python_interface;
#[cfg(test)]
mod tests;
mod thread_bundle_map;
mod update_check;
mod websocket;

use crate::bundle_manager::BundleManager;
use crate::config::read_client_config;
use crate::daemon::daemonize_with_log_redirect;
use crate::logging::init_default_logging;
use crate::update_check::check_for_updates;
use crate::websocket::get_websocket_client;
use std::env;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};

fn get_executable_path() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("./adacs_job_client"))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <libpython_path> [websocket_token]", args[0]);
        eprintln!("  libpython_path: Path to the Python library (required)");
        eprintln!("  websocket_token: WebSocket authentication token (optional, can also use ADACS_LTK env var)");
        eprintln!();
        eprintln!("Set ADACS_LTK environment variable with your LTK token if not providing it as argument.");
        std::process::exit(1);
    }

    let libpython_path = &args[1];

    // Get WebSocket token from either CLI argument or environment variable
    let ws_token = if args.len() >= 3 {
        info!("Using WebSocket token from command line argument");
        args[2].clone()
    } else {
        info!("Using WebSocket token from ADACS_LTK environment variable");
        env::var("ADACS_LTK")
            .expect("ADACS_LTK environment variable must be set with your LTK token (or provide token as CLI argument)")
    };

    // Check for updates BEFORE daemonizing (like C++)
    // This will download update and restart if available
    check_for_updates();

    // Initialize logging with rotating file appender (GLOG replacement)
    let executable_path = get_executable_path();
    init_default_logging(&executable_path);

    info!("ADACS Job Controller Client starting...");

    // Daemonize (UNIX double-fork) with stdout/stderr redirection - matches C++ implementation exactly
    // C++ redirects stdout/stderr to files AFTER forking (main.cpp lines 118-119)
    let log_dir = executable_path.join("logs");
    match daemonize_with_log_redirect(&log_dir) {
        Ok(true) => {
            // We are the daemon process - continue
            info!(
                "Running as daemon process with stdout/stderr redirected to {:?}",
                log_dir
            );
        }
        Ok(false) => {
            // We are a parent process - exit
            return Ok(());
        }
        Err(e) => {
            error!("Failed to daemonize: {}. Running in foreground mode.", e);
        }
    }

    // Initialize Python
    python_interface::load_python_library(libpython_path);
    unsafe {
        python_interface::PyImport_AppendInittab(
            b"_bundledb\0".as_ptr() as *const std::os::raw::c_char,
            Some(bundle_db::PyInit_bundledb),
        );
        python_interface::PyImport_AppendInittab(
            b"_bundlelogging\0".as_ptr() as *const std::os::raw::c_char,
            Some(bundle_logging::PyInit_bundlelogging),
        );
    }
    python_interface::init_python();

    // Initialize Database
    db::initialize("sqlite:db.sqlite3?mode=rwc").await?;

    // Initialize BundleManager
    let bundle_path = crate::bundle_manager::get_default_bundle_path();
    BundleManager::initialize(bundle_path);

    // Get config for WebSocket endpoint
    let config = read_client_config();
    let ws_endpoint = config["websocketEndpoint"]
        .as_str()
        .unwrap_or("ws://127.0.0.1:8001/ws/");
    let ws_url = if ws_endpoint.ends_with('/') {
        ws_endpoint.to_string()
    } else {
        format!("{}/", ws_endpoint)
    };

    info!("Connecting to WebSocket endpoint: {}", ws_url);

    // Start WebSocket client with token
    let ws_client = get_websocket_client();
    ws_client.start_with_token(ws_url, ws_token).await?;

    // Wait for server to be ready
    while !ws_client.is_server_ready() {
        sleep(Duration::from_millis(100)).await;
    }

    info!("Server ready, starting job status check loop");

    // Run loop - check jobs every 60 seconds (matching C++ JOB_CHECK_SECONDS)
    loop {
        jobs::check_all_jobs_status().await;
        sleep(Duration::from_secs(60)).await;
    }
}
