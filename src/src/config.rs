use crate::bundle_manager::get_executable_path;
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufReader;

#[cfg(test)]
lazy_static::lazy_static! {
    pub static ref TEST_CONFIG: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);
}

pub fn read_client_config() -> Value {
    #[cfg(test)]
    {
        if let Some(config) = TEST_CONFIG.lock().unwrap().as_ref() {
            return config.clone();
        }
    }

    let config_path = get_executable_path().join("config.json");
    if let Ok(file) = File::open(config_path) {
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).unwrap_or(json!({}))
    } else {
        json!({})
    }
}

/// Get the Python library path from config or environment variable
pub fn get_python_library_path() -> String {
    // First check environment variable (highest priority)
    if let Ok(path) = std::env::var("PYTHON_LIB_PATH") {
        return path;
    }

    // Then check config file
    let config = read_client_config();
    if let Some(path) = config.get("pythonLibrary").and_then(|v| v.as_str()) {
        return path.to_string();
    }

    // Default fallback
    "libpython3.so".to_string()
}
