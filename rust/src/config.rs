use serde_json::{json, Value};
use std::fs::File;
use std::io::BufReader;
use crate::bundle_manager::get_executable_path;

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
