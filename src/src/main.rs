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
use crate::config::{
    get_log_level, get_ltk_from_config, get_python_library_path, read_client_config,
    validate_config,
};
use crate::daemon::daemonize_with_log_redirect;
use crate::logging::init_logging_with_level;
use crate::update_check::check_for_updates;
use crate::websocket::{get_reconnect_notify, get_shutdown_notify, get_websocket_client};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;
use tracing::{debug, error, info, trace, warn};

const _GIT_HASH: &str = env!("GIT_HASH");

static IS_LTK: AtomicBool = AtomicBool::new(false);
static READY_FOR_RESTART: AtomicBool = AtomicBool::new(false);

fn is_production() -> bool {
    #[cfg(debug_assertions)]
    {
        false
    }
    #[cfg(not(debug_assertions))]
    {
        true
    }
}

pub fn restart_app() {
    let args: Vec<String> = env::args().collect();
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("./adacs_job_client"));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(&args[1..]);
        // exec() replaces the current process. If it returns, it failed.
        let err = cmd.exec();
        eprintln!("Failed to restart app via exec: {err}");
    }

    #[cfg(not(unix))]
    {
        if std::process::Command::new(exe)
            .args(&args[1..])
            .spawn()
            .is_ok()
        {
            std::process::exit(1);
        }
    }
}

fn resolve_websocket_token(
    args: &[String],
    config: &serde_json::Value,
) -> Result<(String, bool), String> {
    let config_ltk = get_ltk_from_config(config);
    let cli_token = args
        .get(1)
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
    // Install the rustls crypto provider before any TLS operation.
    // Both aws-lc-rs and ring features may be enabled transitively (ureq,
    // tokio-rustls, etc.), so auto-detection cannot decide which to use.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls aws-lc-rs crypto provider");

    // Install panic hook before anything else — writes to stderr before abort
    // so panics are visible even with panic = "abort" in release profile.
    std::panic::set_hook(Box::new(|panic_info| {
        let msg = match panic_info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => panic_info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
        };
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        eprintln!("FATAL PANIC: {msg} at {location}");
        let _ = std::io::Write::flush(&mut std::io::stderr());

        if is_production()
            && IS_LTK.load(Ordering::SeqCst)
            && READY_FOR_RESTART.load(Ordering::SeqCst)
        {
            eprintln!("Restarting application due to fatal panic in production with LTK...");
            restart_app();
        }
    }));

    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        eprintln!("Usage: adacs_job_client [websocket_token]");
        eprintln!(
            "  websocket_token: One-shot WebSocket token (optional when config.json 'ltk' is set)"
        );
        eprintln!();
        eprintln!("Configure a persistent LTK in config.json for automatic reconnect support.");
        std::process::exit(0);
    }

    let executable_path = bundle_manager::get_executable_path();
    let log_dir = executable_path.join("logs");

    let config = read_client_config();
    let log_level = get_log_level(&config);
    init_logging_with_level(&executable_path, &log_level);
    info!(
        "ADACS Job Controller Client v{} ({}) starting...",
        env!("CARGO_PKG_VERSION"),
        env!("GIT_HASH")
    );
    info!(
        "Log level: {} (override with RUST_LOG environment variable)",
        log_level
    );
    info!(
        "Logging initialized with debug level - check logs at {}",
        log_dir.display()
    );

    check_for_updates();

    // Daemonize BEFORE creating the tokio runtime — fork() in a multi-threaded
    // process is undefined behaviour (worker threads are silently destroyed).
    match daemonize_with_log_redirect(&log_dir) {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(e) => {
            error!("Failed to daemonize: {}. Running in foreground mode.", e);
        }
    }

    // Create tokio runtime after daemonization — single-threaded process at this point
    let rt = Runtime::new()?;
    let result = rt.block_on(async {
        info!(
            "Using config file: {}",
            executable_path.join("config.json").display()
        );
        if let Err(errors) = validate_config(&config) {
            for e in &errors {
                error!("Config validation error: {e}");
            }
            std::process::exit(1);
        }
        info!("Python library: {}", get_python_library_path());
        let (ws_token, reconnectable) = match resolve_websocket_token(&args, &config) {
            Ok((token, is_ltk)) => {
                IS_LTK.store(is_ltk, Ordering::SeqCst);
                (token, is_ltk)
            }
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

        if let Err(e) = python_interface::load_python_library(&get_python_library_path()) {
            error!("Failed to load Python library: {e}");
            std::process::exit(1);
        }
        // SAFETY: PyImport_AppendInittab registers built-in extension modules before
        // Py_Initialize. Module names are valid null-terminated C strings and init_func
        // pointers reference `#[no_mangle]` C-ABI entry points in bundle_db and
        // bundle_logging. The Python shared library was loaded above; init_python() has
        // not yet been called.
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

        let ws_endpoint = config["websocketEndpoint"].as_str().unwrap();
        let ws_url = if ws_endpoint.ends_with('/') {
            ws_endpoint.to_string()
        } else {
            format!("{ws_endpoint}/")
        };

        info!("Connecting to WebSocket endpoint: {}", ws_url);

        let ws_client = get_websocket_client();
        let shutdown_notify = get_shutdown_notify();
        let reconnect_notify = get_reconnect_notify();
        debug!(
            "WS: Calling start_with_token - reconnectable={}",
            reconnectable
        );
        ws_client
            .start_with_token(ws_url, ws_token, reconnectable)
            .await?;
        debug!("WS: start_with_token completed");

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
        READY_FOR_RESTART.store(true, Ordering::SeqCst);

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
                    let connected = !ws_client.is_connection_closed();
                    let ready = ws_client.is_server_ready();
                    trace!("Main loop tick - connected={}, ready={}", connected, ready);
                    if ws_client.is_connection_closed() {
                        if reconnectable {
                            warn!("WebSocket connection closed, waiting for reconnect...");
                            continue;
                        }

                        error!("WebSocket connection closed without configured LTK; shutting down");
                        break;
                    }
                    debug!("Main loop: running job status check");
                    jobs::check_all_jobs_status().await;
                }
                () = reconnect_notify.notified() => {
                    debug!("Main loop: received reconnect notification");
                    if ws_client.is_connection_closed() {
                        debug!("Main loop: skipping status check - still disconnected");
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
