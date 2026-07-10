//! Auto-update module that checks GitHub releases and downloads updates.
//!
//! Mirrors the C++ UpdateCheck.cpp functionality:
//! 1. Checks GitHub API for latest release
//! 2. Compares semantic versions
//! 3. Downloads the new binary if update is available
//! 4. Replaces the current binary and restarts

use semver::Version;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;
use tracing::{debug, error, info, trace, warn};

const GITHUB_ENDPOINT: &str = "api.github.com";
const GITHUB_LATEST_URL: &str =
    "/repos/gravitationalwavedc/adacs_job_controller_client/releases/latest";

const MAX_DOWNLOAD_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_SECS: u64 = 1;

fn get_current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns the path where the update binary should be written.
/// The update file is placed next to the current executable.
fn get_update_path(executable_path: &Path) -> PathBuf {
    executable_path.with_file_name("adacs_job_client.update")
}

/// Computes the delay in seconds for retry attempt N (1-indexed using exponential backoff).
fn retry_delay_secs(attempt: u32) -> u64 {
    INITIAL_RETRY_DELAY_SECS * 2u64.pow(attempt - 1)
}

/// Parse a GitHub releases API JSON response and return the download URL
/// if a newer version is available, or None if already up-to-date.
fn parse_release_response(
    response: &Value,
    current_version: &str,
) -> Result<Option<String>, String> {
    let latest_tag = response["tag_name"].as_str().unwrap_or("");
    let latest_version_str = latest_tag.trim_start_matches('v');

    let current = Version::parse(current_version)
        .map_err(|e| format!("Failed to parse current version '{current_version}': {e}"))?;
    let latest = Version::parse(latest_version_str)
        .map_err(|e| format!("Failed to parse latest version '{latest_version_str}': {e}"))?;

    if latest <= current {
        return Ok(None);
    }

    let download_url = response["assets"][0]["browser_download_url"]
        .as_str()
        .ok_or("No download URL found in GitHub response")?;

    Ok(Some(download_url.to_string()))
}

/// Replace the running binary with downloaded update data.
/// Writes the new binary to a `.update` sibling file, then atomically renames it
/// over the running binary (safe on Linux — kernel keeps the old inode alive).
fn replace_binary(
    executable_path: &Path,
    update_data: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let update_path = get_update_path(executable_path);

    fs::write(&update_path, update_data)?;
    fs::rename(&update_path, executable_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(executable_path, fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

/// Check GitHub for latest release and return download URL if update is available.
fn check_for_update() -> Result<Option<String>, Box<dyn std::error::Error>> {
    let current_version = get_current_version();
    info!("Checking for updates. Current version: {current_version}");
    debug!("Update check: GitHub endpoint={}", GITHUB_ENDPOINT);

    let response = ureq::AgentBuilder::new()
        .user_agent("ADACS-Job-Controller-Client-Update-Check")
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(10))
        .build()
        .get(&format!("https://{GITHUB_ENDPOINT}{GITHUB_LATEST_URL}"))
        .call();

    match response {
        Ok(resp) => {
            let result: Value = resp.into_json()?;
            trace!("Update check: received response: {:?}", result);
            let download_url = parse_release_response(&result, current_version)
                .map_err(|e| format!("Failed to parse release response: {e}"))?;
            if let Some(ref url) = download_url {
                info!("Update check: update available, download URL={}", url);
            } else {
                debug!("Update check: already up to date");
            }
            Ok(download_url)
        }
        Err(e) => {
            error!("Unable to check for update: {e}");
            Ok(None)
        }
    }
}

/// Download file from URL with retry and exponential backoff.
fn download_file(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    info!("Downloading update from: {}", url);
    for attempt in 1..=MAX_DOWNLOAD_RETRIES {
        debug!("Download attempt {attempt}/{MAX_DOWNLOAD_RETRIES}");
        let agent = ureq::AgentBuilder::new()
            .user_agent("ADACS-Job-Controller-Client-Update-Check")
            .redirects(3)
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .build();

        match agent.get(url).call() {
            Ok(resp) => {
                let mut data = Vec::new();
                resp.into_reader().read_to_end(&mut data)?;
                debug!("Download complete: {} bytes", data.len());
                return Ok(data);
            }
            Err(e) if attempt < MAX_DOWNLOAD_RETRIES => {
                let delay = retry_delay_secs(attempt);
                warn!(
                    "Download attempt {attempt}/{MAX_DOWNLOAD_RETRIES} failed: {e}. \
                     Retrying in {delay}s..."
                );
                std::thread::sleep(Duration::from_secs(delay));
            }
            Err(e) => {
                error!("Download failed after {MAX_DOWNLOAD_RETRIES} attempts: {e}");
                return Err(
                    format!("Download failed after {MAX_DOWNLOAD_RETRIES} attempts: {e}").into(),
                );
            }
        }
    }
    unreachable!()
}

/// Perform the update: download, replace binary, and restart.
fn perform_update(download_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let executable_path = std::env::current_exe()?;
    debug!("Performing update: executable_path={:?}", executable_path);
    let update_data = download_file(download_url)?;

    debug!("Replacing binary...");
    replace_binary(&executable_path, &update_data)?;

    info!("Update complete, restarting...");

    crate::restart_app();

    // In case restart_app returns (which it shouldn't on Unix unless exec fails)
    process::exit(0);
}

/// Main update check function — call this at startup before connecting WebSocket.
pub fn check_for_updates() {
    match check_for_update() {
        Ok(Some(download_url)) => {
            if let Err(e) = perform_update(&download_url) {
                error!("Failed to perform update: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => {
            error!("Error checking for updates: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn test_get_update_path() {
        let cases = [
            (
                "/usr/local/bin/adacs_job_client",
                "/usr/local/bin/adacs_job_client.update",
            ),
            (
                "/usr/bin/adacs_job_client",
                "/usr/bin/adacs_job_client.update",
            ),
            ("./adacs_job_client", "./adacs_job_client.update"),
        ];
        for (exe, expected) in &cases {
            let result = get_update_path(&PathBuf::from(exe));
            assert_eq!(result, PathBuf::from(expected), "Failed for {exe}");
        }
    }

    #[test]
    fn test_get_update_path_sibling_not_child() {
        let path = PathBuf::from("/usr/local/bin/adacs_job_client");
        let result = get_update_path(&path);
        assert_eq!(result.parent(), path.parent());
        assert_eq!(result.file_name().unwrap(), "adacs_job_client.update");
    }

    #[test]
    fn test_version_parse() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert!(v.pre.is_empty());
    }

    #[test]
    fn test_v_prefix_handling() {
        let parsed = Version::parse("v1.2.3".trim_start_matches('v')).unwrap();
        assert_eq!(parsed, Version::new(1, 2, 3));
    }

    #[test]
    fn test_version_compare_update_available() {
        assert!(Version::parse("2.0.0").unwrap() > Version::parse("1.0.0").unwrap());
        assert!(Version::parse("1.3.0").unwrap() > Version::parse("1.2.0").unwrap());
        assert!(Version::parse("1.2.4").unwrap() > Version::parse("1.2.3").unwrap());
        assert!(Version::parse("1.2.3").unwrap() <= Version::parse("1.2.3").unwrap());
    }

    #[test]
    fn test_pre_release_version() {
        let release = Version::parse("1.0.0").unwrap();
        let pre_release = Version::parse("1.0.0-rc1").unwrap();
        assert!(release > pre_release);
        assert!(pre_release <= release);
    }

    #[test]
    fn test_build_metadata() {
        let v1 = Version::parse("1.0.0+build123").unwrap();
        let v2 = Version::parse("1.0.0+build456").unwrap();
        assert_eq!(v1.cmp_precedence(&v2), std::cmp::Ordering::Equal);
        assert!(v1 != v2);
    }

    #[test]
    fn test_version_invalid_input() {
        assert!(Version::parse("").is_err());
        assert!(Version::parse("abc").is_err());
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("v1.2.3").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
    }

    #[test]
    fn test_get_current_version() {
        let version = get_current_version();
        assert!(
            Version::parse(version).is_ok(),
            "CARGO_PKG_VERSION '{version}' is not valid semver"
        );
        assert!(!version.is_empty());
    }

    #[test]
    fn test_get_executable_path_exists() {
        let path = std::env::current_exe().unwrap();
        assert!(path.exists());
        assert!(path.is_file());
    }

    #[test]
    fn test_retry_delay_secs() {
        assert_eq!(retry_delay_secs(1), 1);
        assert_eq!(retry_delay_secs(2), 2);
        assert_eq!(retry_delay_secs(3), 4);
        assert_eq!(retry_delay_secs(4), 8);
        assert_eq!(retry_delay_secs(5), 16);
    }

    #[test]
    fn test_retry_constants() {
        const _: () = assert!(MAX_DOWNLOAD_RETRIES >= 1);
        const _: () = assert!(MAX_DOWNLOAD_RETRIES <= 10);
        assert_eq!(INITIAL_RETRY_DELAY_SECS, 1);
    }

    // ─── parse_release_response tests ─────────────────────────────────────

    #[test]
    fn test_parse_release_update_available() {
        let resp = json!({
            "tag_name": "v1.2.0",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "1.1.0").unwrap();
        assert_eq!(
            result,
            Some("https://example.com/adacs_job_client".to_string())
        );
    }

    #[test]
    fn test_parse_release_no_update() {
        let resp = json!({
            "tag_name": "v1.1.0",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "1.1.0").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_release_current_ahead() {
        let resp = json!({
            "tag_name": "v1.0.0",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "2.0.0").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_release_empty_tag() {
        let resp = json!({
            "tag_name": "",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "1.0.0");
        assert!(result.is_err(), "empty tag should fail to parse");
    }

    #[test]
    fn test_parse_release_no_tag() {
        let resp = json!({
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "1.0.0");
        assert!(result.is_err(), "missing tag should fail to parse");
    }

    #[test]
    fn test_parse_release_no_assets() {
        let resp = json!({
            "tag_name": "v2.0.0",
            "assets": []
        });
        let result = parse_release_response(&resp, "1.0.0");
        assert!(result.is_err(), "empty assets should fail: {result:?}");
    }

    #[test]
    fn test_parse_release_pre_release_version() {
        // Pre-release tags — should not auto-upgrade to a lower pre-release
        let resp = json!({
            "tag_name": "v1.0.0-rc1",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "1.0.0").unwrap();
        assert_eq!(
            result, None,
            "should not downgrade from release to pre-release"
        );

        // But should upgrade FROM older version TO pre-release
        let resp2 = json!({
            "tag_name": "v2.0.0-rc1",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result2 = parse_release_response(&resp2, "1.9.0").unwrap();
        assert_eq!(
            result2,
            Some("https://example.com/adacs_job_client".to_string())
        );
    }

    #[test]
    fn test_parse_release_without_v_prefix() {
        let resp = json!({
            "tag_name": "1.2.0",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "1.1.0").unwrap();
        assert_eq!(
            result,
            Some("https://example.com/adacs_job_client".to_string())
        );
    }

    #[test]
    fn test_parse_release_invalid_current_version() {
        let resp = json!({
            "tag_name": "v1.2.0",
            "assets": [{"browser_download_url": "https://example.com/adacs_job_client"}]
        });
        let result = parse_release_response(&resp, "not-a-version");
        assert!(result.is_err(), "invalid current version should error");
    }

    // ─── replace_binary tests ─────────────────────────────────────────────

    #[test]
    fn test_replace_binary_writes_and_renames() {
        let dir = tempfile::TempDir::new().unwrap();
        let exe_path = dir.path().join("adacs_job_client");
        // Create a "running" binary so rename has something to overwrite
        fs::write(&exe_path, b"old binary content").unwrap();

        let update_data = b"new binary content v2.0";
        replace_binary(&exe_path, update_data).unwrap();

        // Verify the update sibling file is GONE (was renamed over the exe)
        let update_path = get_update_path(&exe_path);
        assert!(
            !update_path.exists(),
            "update file should be removed after rename"
        );

        // Verify the executable now has the new content
        let new_content = fs::read(&exe_path).unwrap();
        assert_eq!(
            new_content, update_data,
            "binary should have new content after update"
        );

        // Verify it's executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&exe_path).unwrap();
            assert!(
                meta.permissions().mode() & 0o111 != 0,
                "binary should be executable"
            );
        }
    }

    #[test]
    fn test_replace_binary_creates_update_sibling() {
        let dir = tempfile::TempDir::new().unwrap();
        let exe_path = dir.path().join("adacs_job_client");
        fs::write(&exe_path, b"old").unwrap();

        replace_binary(&exe_path, b"new data").unwrap();

        // The update file should NOT remain as a sibling
        let update_path = get_update_path(&exe_path);
        assert!(
            !update_path.exists(),
            "update file '{}' should be gone after rename",
            update_path.display()
        );
    }

    #[test]
    fn test_replace_binary_fails_on_missing_parent_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        // Parent directory doesn't exist
        let exe_path = dir.path().join("missing_dir").join("adacs_job_client");
        let result = replace_binary(&exe_path, b"data");
        assert!(
            result.is_err(),
            "should fail when parent directory doesn't exist"
        );
    }
}
