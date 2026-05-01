//! Auto-update module that checks GitHub releases and downloads updates.
//!
//! Mirrors the C++ UpdateCheck.cpp functionality:
//! 1. Checks GitHub API for latest release
//! 2. Compares semantic versions
//! 3. Downloads the new binary if update is available
//! 4. Replaces the current binary and exits

use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process;
use tokio::runtime::Runtime;
use tracing::{error, info, warn};

// Constants from C++ Settings.h
const GITHUB_ENDPOINT: &str = "api.github.com";
const GITHUB_LATEST_URL: &str =
    "/repos/gravitationalwavedc/adacs_job_controller_client/releases/latest";

/// Get the current version string (from Cargo.toml or environment)
fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Parse semantic version string into comparable components
fn parse_semver(version: &str) -> Result<(u64, u64, u64), String> {
    let version = version.trim_start_matches('v');
    let parts: Vec<&str> = version.split('.').collect();

    if parts.len() < 3 {
        return Err(format!("Invalid semver format: {version}"));
    }

    let major = parts[0]
        .parse::<u64>()
        .map_err(|e| format!("Invalid major version: {e}"))?;
    let minor = parts[1]
        .parse::<u64>()
        .map_err(|e| format!("Invalid minor version: {e}"))?;
    let patch = parts[2]
        .parse::<u64>()
        .map_err(|e| format!("Invalid patch version: {e}"))?;

    Ok((major, minor, patch))
}

/// Compare two semver strings. Returns true if latest > current
fn is_update_available(current: &str, latest: &str) -> bool {
    if let (Ok((cur_major, cur_minor, cur_patch)), Ok((lat_major, lat_minor, lat_patch))) =
        (parse_semver(current), parse_semver(latest))
    {
        if lat_major != cur_major {
            return lat_major > cur_major;
        }
        if lat_minor != cur_minor {
            return lat_minor > cur_minor;
        }
        lat_patch > cur_patch
    } else {
        warn!("Failed to parse semver, assuming no update available");
        false
    }
}

/// Check GitHub for latest release and return download URL if update is available
fn check_for_update() -> Result<Option<String>, Box<dyn std::error::Error>> {
    let current_version = get_current_version();
    info!("Checking for updates. Current version: {}", current_version);

    // Use ureq for synchronous HTTPS requests
    let response = ureq::AgentBuilder::new()
        .user_agent("ADACS-Job-Controller-Client-Update-Check")
        .build()
        .get(&format!("https://{GITHUB_ENDPOINT}{GITHUB_LATEST_URL}"))
        .call();

    match response {
        Ok(resp) => {
            let result: Value = resp.into_json()?;

            // Extract tag_name and remove 'v' prefix
            let latest_version = result["tag_name"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches('v')
                .to_string();

            info!("Latest version from GitHub: {}", latest_version);

            // Check if update is available
            if !is_update_available(&current_version, &latest_version) {
                info!("No updates available. Continuing...");
                return Ok(None);
            }

            info!(
                "Found update. Local version: {}, latest version: {}",
                current_version, latest_version
            );

            // Get download URL from first asset
            let download_url = result["assets"][0]["browser_download_url"]
                .as_str()
                .ok_or("No download URL found in GitHub response")?;

            info!("Downloading update from url: {}", download_url);

            Ok(Some(download_url.to_string()))
        }
        Err(e) => {
            error!("Unable to check for update: {}", e);
            Ok(None)
        }
    }
}

/// Download file from URL
fn download_file(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    info!("Downloading from: {}", url);

    // Use ureq for synchronous HTTPS requests with redirect handling
    let response = ureq::AgentBuilder::new()
        .user_agent("ADACS-Job-Controller-Client-Update-Check")
        .redirects(3) // Handle up to 3 redirects (like C++)
        .build()
        .get(url)
        .call();

    match response {
        Ok(resp) => {
            let mut data = Vec::new();
            let mut reader = resp.into_reader();
            reader.read_to_end(&mut data)?;
            Ok(data)
        }
        Err(e) => {
            error!("Unable to download updated binary: {}", e);
            Err(format!("Download failed: {e}").into())
        }
    }
}

/// Get the path to the current executable
fn get_executable_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("./adacs_job_client"))
}

/// Perform the update: download, replace binary, and restart
fn perform_update(download_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let executable_path = get_executable_path();
    let update_path = executable_path.with_extension("adacs_job_client.update");
    let old_path = &executable_path;

    info!("Downloading update...");
    let update_data = download_file(download_url)?;

    info!("Writing update to disk...");
    let mut out_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&update_path)?;
    out_file.write_all(&update_data)?;
    out_file.flush()?;
    drop(out_file);

    info!("Download finished, moving new binary in place and restarting");

    // Move update file to executable path (with retry loop like C++)
    // Note: In Rust, we can't easily do the shell loop, so we use a simple move
    fs::rename(&update_path, old_path)?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(old_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(old_path, perms)?;
    }

    info!("Update complete, restarting...");

    // Restart the application
    let args: Vec<String> = std::env::args().collect();
    process::Command::new(&executable_path)
        .args(&args[1..])
        .spawn()?;

    // Exit current process
    process::exit(0);
}

/// Main update check function - call this at startup before connecting WebSocket
pub fn check_for_updates() {
    // Use a blocking runtime for the update check (before tokio::main starts)
    let rt = Runtime::new().expect("Failed to create runtime");

    rt.block_on(async {
        match check_for_update() {
            Ok(Some(download_url)) => {
                if let Err(e) = perform_update(&download_url) {
                    error!("Failed to perform update: {}", e);
                }
            }
            Ok(None) => {
                // No update available or error occurred
            }
            Err(e) => {
                error!("Error checking for updates: {}", e);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("1.2.3"), Ok((1, 2, 3)));
        assert_eq!(parse_semver("v1.2.3"), Ok((1, 2, 3)));
        assert_eq!(parse_semver("0.0.1"), Ok((0, 0, 1)));
    }

    #[test]
    fn test_is_update_available() {
        // Major version updates
        assert!(is_update_available("1.0.0", "2.0.0"));
        assert!(!is_update_available("2.0.0", "1.0.0"));

        // Minor version updates
        assert!(is_update_available("1.2.0", "1.3.0"));
        assert!(!is_update_available("1.3.0", "1.2.0"));

        // Patch version updates
        assert!(is_update_available("1.2.3", "1.2.4"));
        assert!(!is_update_available("1.2.4", "1.2.3"));

        // Same version
        assert!(!is_update_available("1.2.3", "1.2.3"));

        // With 'v' prefix
        assert!(is_update_available("v1.2.3", "v1.2.4"));
    }
}
