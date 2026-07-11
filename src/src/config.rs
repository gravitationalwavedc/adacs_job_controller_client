use crate::bundle_manager::get_executable_path;
use serde_json::Value;
use std::fs::File;
use std::io::BufReader;
#[cfg(test)]
use std::sync::LazyLock;

#[cfg(test)]
pub static TEST_CONFIG: LazyLock<std::sync::Mutex<Option<Value>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

pub fn read_client_config() -> Value {
    #[cfg(test)]
    {
        if let Some(config) = TEST_CONFIG.lock().unwrap().as_ref() {
            return config.clone();
        }
    }

    let mut config_path = get_executable_path();
    config_path.pop(); // Remove binary name
    config_path.push("config.json");
    let path_str = config_path.to_string_lossy().to_string();

    let Ok(file) = File::open(&config_path) else {
        eprintln!("Error: config.json not found at {path_str}");
        std::process::exit(1);
    };

    let reader = BufReader::new(file);
    match serde_json::from_reader(reader) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("Error: config.json parse error at {path_str}: {e}");
            std::process::exit(1);
        }
    }
}

pub fn validate_config(config: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    match config.get("pythonLibrary") {
        None | Some(Value::Null) => {
            errors.push("pythonLibrary is missing or null".to_string());
        }
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                errors.push("pythonLibrary is empty".to_string());
            }
        }
        Some(_) => {
            errors.push("pythonLibrary must be a string".to_string());
        }
    }

    match config.get("websocketEndpoint") {
        None | Some(Value::Null) => {
            errors.push("websocketEndpoint is missing or null".to_string());
        }
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                errors.push("websocketEndpoint is empty".to_string());
            }
        }
        Some(_) => {
            errors.push("websocketEndpoint must be a string".to_string());
        }
    }

    // Optional: validate logLevel if present
    if let Some(log_level) = config.get("logLevel") {
        if let Some(s) = log_level.as_str() {
            if s.trim().is_empty() {
                errors.push("logLevel is empty".to_string());
            }
        } else if !log_level.is_null() {
            errors.push("logLevel must be a string".to_string());
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn get_ltk_from_config(config: &Value) -> Option<String> {
    config
        .get("ltk")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Get the Python library path from config or environment variable
pub fn get_python_library_path() -> String {
    // First check environment variable (highest priority)
    if let Ok(path) = std::env::var("PYTHON_LIB_PATH") {
        return path;
    }

    // Then check config file
    let config = read_client_config();
    if let Some(path) = config
        .get("pythonLibrary")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return path.to_string();
    }

    eprintln!("Error: pythonLibrary not found in config.json and PYTHON_LIB_PATH environment variable is not set");
    std::process::exit(1);
}

/// Ensure the WebSocket endpoint URL ends with a trailing slash.
pub fn ensure_websocket_endpoint_trailing_slash(endpoint: &str) -> String {
    if endpoint.ends_with('/') {
        endpoint.to_string()
    } else {
        format!("{endpoint}/")
    }
}

/// Get the log level from config, defaulting to "info" if not set
pub fn get_log_level(config: &Value) -> String {
    let result = config
        .get("logLevel")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| "info".to_string(), ToOwned::to_owned);
    tracing::debug!("Config log level: {}", result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reset_test_config() {
        *TEST_CONFIG.lock().unwrap() = None;
    }

    fn set_test_config(value: Value) {
        *TEST_CONFIG.lock().unwrap() = Some(value);
    }

    // --- validate_config tests ---

    #[test]
    fn validate_config_accepts_valid_config() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://example.com/ws/"
        });
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_config_rejects_missing_python_library() {
        let config = json!({"websocketEndpoint": "ws://example.com/ws/"});
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("pythonLibrary")));
    }

    #[test]
    fn validate_config_rejects_missing_websocket_endpoint() {
        let config = json!({"pythonLibrary": "/usr/lib/libpython3.so"});
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("websocketEndpoint")));
    }

    #[test]
    fn validate_config_rejects_both_missing() {
        let config = json!({});
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.len(), 2);
        assert!(err.iter().any(|e| e.contains("pythonLibrary")));
        assert!(err.iter().any(|e| e.contains("websocketEndpoint")));
    }

    #[test]
    fn validate_config_rejects_empty_values() {
        let config = json!({
            "pythonLibrary": "",
            "websocketEndpoint": ""
        });
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.len(), 2);
    }

    #[test]
    fn validate_config_rejects_non_string_types() {
        let config = json!({
            "pythonLibrary": 42,
            "websocketEndpoint": true
        });
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.len(), 2);
        assert!(err.iter().any(|e| e.contains("must be a string")));
    }

    #[test]
    fn validate_config_rejects_null_values() {
        let config = json!({
            "pythonLibrary": null,
            "websocketEndpoint": null
        });
        let err = validate_config(&config).unwrap_err();
        assert_eq!(err.len(), 2);
    }

    // --- read_client_config tests ---

    #[test]
    #[serial_test::serial]
    fn read_client_config_returns_test_config() {
        let expected =
            json!({"pythonLibrary": "/test/libpython.so", "websocketEndpoint": "ws://test/ws/"});
        set_test_config(expected.clone());
        let result = read_client_config();
        assert_eq!(result, expected);
        reset_test_config();
    }

    // --- get_python_library_path tests ---

    #[test]
    #[serial_test::serial]
    fn get_python_library_path_env_var_and_config_fallback() {
        // Save existing env so this test does not leak state to later
        // tests / spawned children (e.g. test-fork re-execs the binary and
        // inherits the parent's env).
        let saved_env = std::env::var("PYTHON_LIB_PATH").ok();

        // Test 1: PYTHON_LIB_PATH env var takes priority
        set_test_config(json!({"pythonLibrary": "/config/libpython.so"}));
        std::env::set_var("PYTHON_LIB_PATH", "/env/libpython.so");
        let result = get_python_library_path();
        std::env::remove_var("PYTHON_LIB_PATH");
        reset_test_config();
        assert_eq!(result, "/env/libpython.so");

        // Test 2: Falls back to config when env var is unset
        set_test_config(json!({"pythonLibrary": "/config/libpython.so"}));
        let result = get_python_library_path();
        reset_test_config();
        assert_eq!(result, "/config/libpython.so");

        // Test 3: Trims whitespace from config value
        set_test_config(json!({"pythonLibrary": "  /config/libpython.so  "}));
        let result = get_python_library_path();
        reset_test_config();
        assert_eq!(result, "/config/libpython.so");

        // Restore the original env (or leave it unset if there was none).
        if let Some(v) = saved_env {
            std::env::set_var("PYTHON_LIB_PATH", v);
        }
    }

    // --- ensure_websocket_endpoint_trailing_slash tests ---

    #[test]
    fn ensure_websocket_endpoint_trailing_slash_preserves_existing_slash() {
        assert_eq!(
            ensure_websocket_endpoint_trailing_slash("ws://example.com/ws/"),
            "ws://example.com/ws/"
        );
    }

    #[test]
    fn ensure_websocket_endpoint_trailing_slash_adds_missing_slash() {
        assert_eq!(
            ensure_websocket_endpoint_trailing_slash("ws://example.com/ws"),
            "ws://example.com/ws/"
        );
    }

    // --- get_log_level tests ---

    #[test]
    fn get_log_level_returns_config_value() {
        let config = json!({"logLevel": "debug"});
        assert_eq!(get_log_level(&config), "debug");
    }

    #[test]
    fn get_log_level_trims_whitespace() {
        let config = json!({"logLevel": "  debug  "});
        assert_eq!(get_log_level(&config), "debug");
    }

    #[test]
    fn get_log_level_rejects_empty_string() {
        let config = json!({"logLevel": ""});
        assert_eq!(get_log_level(&config), "info");
    }

    #[test]
    fn get_log_level_whitespace_only_defaults_to_info() {
        let config = json!({"logLevel": "   "});
        assert_eq!(get_log_level(&config), "info");
    }

    #[test]
    fn get_log_level_defaults_to_info_when_missing() {
        let config = json!({});
        assert_eq!(get_log_level(&config), "info");
    }

    #[test]
    fn get_log_level_defaults_to_info_when_null() {
        let config = json!({"logLevel": null});
        assert_eq!(get_log_level(&config), "info");
    }

    // --- get_ltk_from_config tests ---

    #[test]
    fn get_ltk_from_config_returns_config_value() {
        let config = json!({"ltk": "my-ltk-token"});
        assert_eq!(
            get_ltk_from_config(&config),
            Some("my-ltk-token".to_string())
        );
    }

    #[test]
    fn get_ltk_from_config_trims_whitespace() {
        let config = json!({"ltk": "  my-ltk-token  "});
        assert_eq!(
            get_ltk_from_config(&config),
            Some("my-ltk-token".to_string())
        );
    }

    #[test]
    fn get_ltk_from_config_rejects_empty_string() {
        let config = json!({"ltk": ""});
        assert_eq!(get_ltk_from_config(&config), None);
    }

    #[test]
    fn get_ltk_from_config_returns_none_when_missing() {
        let config = json!({});
        assert_eq!(get_ltk_from_config(&config), None);
    }

    #[test]
    fn get_ltk_from_config_returns_none_when_null() {
        let config = json!({"ltk": null});
        assert_eq!(get_ltk_from_config(&config), None);
    }

    #[test]
    fn get_log_level_defaults_to_info_for_non_string() {
        let config = json!({"logLevel": 42});
        assert_eq!(get_log_level(&config), "info");
    }

    #[test]
    fn validate_config_accepts_valid_log_level() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://example.com/ws/",
            "logLevel": "debug"
        });
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_config_rejects_empty_log_level() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://example.com/ws/",
            "logLevel": ""
        });
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("logLevel is empty")));
    }

    #[test]
    fn validate_config_rejects_non_string_log_level() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://example.com/ws/",
            "logLevel": 42
        });
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("logLevel must be a string")));
    }

    #[test]
    fn validate_config_accepts_missing_log_level() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://example.com/ws/"
        });
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_config_rejects_whitespace_only_python_library() {
        let config = json!({
            "pythonLibrary": "   ",
            "websocketEndpoint": "ws://example.com/ws/"
        });
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("pythonLibrary is empty")));
    }

    #[test]
    fn validate_config_rejects_whitespace_only_websocket_endpoint() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "  \t  "
        });
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("websocketEndpoint is empty")));
    }

    #[test]
    fn validate_config_rejects_whitespace_only_log_level() {
        let config = json!({
            "pythonLibrary": "/usr/lib/libpython3.so",
            "websocketEndpoint": "ws://example.com/ws/",
            "logLevel": "   "
        });
        let err = validate_config(&config).unwrap_err();
        assert!(err.iter().any(|e| e.contains("logLevel is empty")));
    }
}
