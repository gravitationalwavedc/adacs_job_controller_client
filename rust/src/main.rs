mod websocket;
mod python_interface;
mod thread_bundle_map;
mod bundle_logging;
mod bundle_interface;
mod bundle_manager;
mod db;
mod bundle_db;
mod messaging;
mod config;
mod jobs;
mod files;
#[cfg(test)]
mod tests;

use std::env;
use std::time::Duration;
use tokio::time::sleep;
use crate::websocket::get_websocket_client;
use crate::bundle_manager::{BundleManager, get_executable_path};
use crate::config::read_client_config;
use std::fs::File;
use std::io::BufReader;
use serde_json::{json, Value};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <ws_token> <libpython_path>", args[0]);
        std::process::exit(1);
    }

    let ws_token = &args[1];
    let libpython_path = &args[2];

    // Initialize logging
    env_logger::init();

    // Initialize Python
    python_interface::load_python_library(libpython_path);
    unsafe {
        python_interface::PyImport_AppendInittab(b"_bundledb\0".as_ptr() as *const std::os::raw::c_char, Some(bundle_db::PyInit_bundledb));
        python_interface::PyImport_AppendInittab(b"_bundlelogging\0".as_ptr() as *const std::os::raw::c_char, Some(bundle_logging::PyInit_bundlelogging));
    }
    python_interface::init_python();

    // Initialize Database
    db::initialize("sqlite:db.sqlite3?mode=rwc").await?;

    // Initialize BundleManager
    let bundle_path = crate::bundle_manager::get_default_bundle_path();
    BundleManager::initialize(bundle_path);

    // Get config for WebSocket endpoint
    let config = read_client_config();
    let ws_endpoint = config["websocketEndpoint"].as_str().unwrap_or("ws://127.0.0.1:8001/ws/");
    let ws_url = format!("{}{}{}", ws_endpoint, if ws_endpoint.ends_with('/') { "" } else { "/" }, ws_token);

    // Start WebSocket client
    let ws_client = get_websocket_client();
    ws_client.start(ws_url).await?;

    // Wait for server to be ready
    while !ws_client.is_server_ready() {
        sleep(Duration::from_millis(100)).await;
    }

    log::info!("Server ready, starting job status check loop");

    // Run loop
    loop {
        jobs::check_all_jobs_status().await;
        sleep(Duration::from_secs(10)).await;
    }
}
