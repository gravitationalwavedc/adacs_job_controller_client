use crate::bundle_interface::BundleInterface;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use log::info;
use serde_json::Value;
use std::sync::Arc;

pub struct BundleManager {
    bundle_path_root: String,
    bundles: RwLock<HashMap<String, BundleInterface>>,
}

static mut SINGLETON: *const BundleManager = std::ptr::null();

impl BundleManager {
    pub fn initialize(bundle_path_root: String) {
        let manager = Box::new(BundleManager {
            bundle_path_root,
            bundles: RwLock::new(HashMap::new()),
        });
        unsafe {
            SINGLETON = Box::into_raw(manager);
        }
    }

    pub fn singleton() -> &'static BundleManager {
        unsafe {
            if SINGLETON.is_null() {
                panic!("BundleManager not initialized");
            }
            &*SINGLETON
        }
    }

    pub fn get_bundle_path(&self, bundle_hash: &str) -> PathBuf {
        Path::new(&self.bundle_path_root).join(bundle_hash)
    }

    pub unsafe fn load_bundle(&self, bundle_hash: &str) -> BundleInterface {
        {
            let bundles = self.bundles.read();
            if let Some(bundle) = bundles.get(bundle_hash) {
                return bundle.clone();
            }
        }

        let bundle_path = self.get_bundle_path(bundle_hash);
        let bundle = unsafe { BundleInterface::new("bundle", bundle_path.to_str().unwrap()) };
        let mut bundles = self.bundles.write();
        // Double check after acquiring write lock
        if let Some(existing) = bundles.get(bundle_hash) {
            return existing.clone();
        }
        bundles.insert(bundle_hash.to_string(), bundle.clone());
        bundle
    }

    pub unsafe fn run_bundle_json(&self, function_name: &str, bundle_hash: &str, details: &Value, _job_data: &str) -> Value {
        let bundle = unsafe { self.load_bundle(bundle_hash) };
        unsafe { bundle.run_json(function_name, details) }
    }

    pub unsafe fn run_bundle_string(&self, function_name: &str, bundle_hash: &str, details: &Value, source: &str) -> String {
        let bundle = unsafe { self.load_bundle(bundle_hash) };
        unsafe { bundle.run_string(function_name, details, source) }
    }

    pub unsafe fn run_bundle_uint64(&self, function_name: &str, bundle_hash: &str, details: &Value, job_data: &str) -> u64 {
        let bundle = unsafe { self.load_bundle(bundle_hash) };
        unsafe {
            let result_obj = bundle.run(function_name, details, job_data);
            let result = bundle.to_uint64(result_obj);
            bundle.dispose_object(result_obj);
            result
        }
    }

    pub unsafe fn run_bundle_bool(&self, function_name: &str, bundle_hash: &str, details: &Value, job_data: &str) -> bool {
        let bundle = unsafe { self.load_bundle(bundle_hash) };
        unsafe {
            let result_obj = bundle.run(function_name, details, job_data);
            let result = bundle.to_bool(result_obj);
            bundle.dispose_object(result_obj);
            result
        }
    }
}

pub fn get_executable_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

pub fn get_default_bundle_path() -> String {
    let mut path = get_executable_path();
    path.pop(); // Remove executable name
    path.push("bundles");
    path.to_string_lossy().to_string()
}
