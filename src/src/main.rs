mod bundle_db;
mod bundle_interface;
mod bundle_logging;
mod bundle_manager;
mod config;
mod daemon;
mod db;
mod db_bridge;
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
use crate::config::{get_ltk_from_config, read_client_config};
use crate::daemon::daemonize_with_log_redirect;
use crate::logging::init_default_logging;
use crate::update_check::check_for_updates;
use crate::websocket::{get_reconnect_notify, get_shutdown_notify, get_websocket_client};
use std::env;
use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Runtime;
use tracing::{error, info, warn};

fn get_executable_path() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("./adacs_job_client"))
        .parent()
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
}

fn resolve_websocket_token(
    args: &[String],
    config: &serde_json::Value,
) -> Result<(String, bool), String> {
    let config_ltk = get_ltk_from_config(config);
    let cli_token = args
        .get(2)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    match (config_ltk, cli_token) {
        (Some(_), Some(_)) => Err(
            "Both config.json 'ltk' and a CLI WebSocket token were provided. Remove one."
                .to_string(),
        ),
        (Some(token), None) => Ok((token, true)),
        (None, Some(token)) => Ok((token.to_string(), false)),
        (None, None) => Err(
            "No WebSocket token configured. Set config.json 'ltk' or pass a CLI token.".to_string(),
        ),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <libpython_path> [websocket_token]", args[0]);
        eprintln!("  libpython_path: Path to the Python library (required)");
        eprintln!(
            "  websocket_token: One-shot WebSocket token (optional when config.json 'ltk' is set)"
        );
        eprintln!();
        eprintln!("Configure a persistent LTK in config.json for automatic reconnect support.");
        std::process::exit(1);
    }

    let libpython_path = &args[1];

    check_for_updates();

    let executable_path = get_executable_path();
    let log_dir = executable_path.join("logs");

    // Daemonize BEFORE creating the tokio runtime — fork() in a multi-threaded
    // process is undefined behaviour (worker threads are silently destroyed).
    match daemonize_with_log_redirect(&log_dir) {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(e) => {
            error!("Failed to daemonize: {}. Running in foreground mode.", e);
        }
    }

    init_default_logging(&executable_path);
    info!("ADACS Job Controller Client starting...");

    // Create tokio runtime after daemonization — single-threaded process at this point
    let rt = Runtime::new()?;
    let result = rt.block_on(async {
        let config = read_client_config();
        let (ws_token, reconnectable) = match resolve_websocket_token(&args, &config) {
            Ok(value) => value,
            Err(message) => {
                error!("{message}");
                eprintln!("{message}");
                std::process::exit(1);
            }
        };

        if reconnectable {
            info!("Using persistent WebSocket token from config.json");
        } else {
            info!("Using one-shot WebSocket token from command line argument");
        }

        python_interface::load_python_library(libpython_path);
        unsafe {
            python_interface::PyImport_AppendInittab(
                c"_bundledb".as_ptr(),
                Some(bundle_db::PyInit_bundledb),
            );
            python_interface::PyImport_AppendInittab(
                c"_bundlelogging".as_ptr(),
                Some(bundle_logging::PyInit_bundlelogging),
            );
        }
        python_interface::init_python();

        let bundle_path = crate::bundle_manager::get_default_bundle_path();
        BundleManager::initialize(bundle_path);

        crate::db_bridge::DbBridge::start();

        let ws_endpoint = config["websocketEndpoint"]
            .as_str()
            .unwrap_or("ws://127.0.0.1:8001/ws/");
        let ws_url = if ws_endpoint.ends_with('/') {
            ws_endpoint.to_string()
        } else {
            format!("{ws_endpoint}/")
        };

        info!("Connecting to WebSocket endpoint: {}", ws_url);

        let ws_client = get_websocket_client();
        let shutdown_notify = get_shutdown_notify();
        let reconnect_notify = get_reconnect_notify();
        ws_client
            .start_with_token(ws_url, ws_token, reconnectable)
            .await?;

        info!("Waiting for server to be ready...");
        let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if ws_client.is_server_ready() {
                break;
            }
            if !reconnectable && ws_client.is_connection_closed() {
                error!("WebSocket connection closed before server ready signal");
                return Err("WebSocket connection closed during startup".into());
            }
            if !reconnectable && tokio::time::Instant::now() > ready_deadline {
                error!("Timed out waiting for server to be ready after 30s");
                return Err("Server did not become ready within the timeout period".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        info!("Server ready, starting job status check loop");

        let mut term_signal =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|e| format!("Failed to set up SIGTERM handler: {e}"))?;
        let mut int_signal =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .map_err(|e| format!("Failed to set up SIGINT handler: {e}"))?;

        let mut job_check_interval = tokio::time::interval(Duration::from_mins(1));
        loop {
            tokio::select! {
                _ = job_check_interval.tick() => {
                    if ws_client.is_connection_closed() {
                        if reconnectable {
                            warn!("WebSocket connection closed, waiting for reconnect...");
                            continue;
                        }

                        error!("WebSocket connection closed without configured LTK; shutting down");
                        break;
                    }
                    jobs::check_all_jobs_status().await;
                }
                () = reconnect_notify.notified() => {
                    if ws_client.is_connection_closed() {
                        continue;
                    }
                    jobs::check_all_jobs_status().await;
                }
                () = shutdown_notify.notified() => {
                    error!("WebSocket client requested shutdown");
                    break;
                }
                _ = term_signal.recv() => {
                    info!("Received SIGTERM, shutting down gracefully");
                    break;
                }
                _ = int_signal.recv() => {
                    info!("Received SIGINT, shutting down gracefully");
                    break;
                }
            }
        }

        info!("Daemon shutting down");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    rt.shutdown_timeout(Duration::from_secs(10));
    result
}
