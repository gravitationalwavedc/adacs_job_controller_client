//! Port of C++ BundleManager.
//!
//! Manages loading and caching of BundleInterface instances (one per bundle hash).
//! All runBundle_* methods acquire the PYTHON_MUTEX, create a ThreadScope, call
//! the bundle function, convert the result, and dispose the PyObject.

use crate::bundle_interface::{BundleInterface, NoneException};
use crate::python_interface::*;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

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

    /// Load (or return cached) BundleInterface for a given hash.
    /// Mirrors C++ BundleManager::loadBundle().
    pub unsafe fn load_bundle(&self, bundle_hash: &str) -> BundleInterface {
        // Check if already loaded (read lock)
        {
            let bundles = self.bundles.read();
            if let Some(bundle) = bundles.get(bundle_hash) {
                return bundle.clone();
            }
        }

        // Not loaded – acquire write lock and create
        let mut bundles = self.bundles.write();
        // Double-check after acquiring write lock
        if let Some(existing) = bundles.get(bundle_hash) {
            return existing.clone();
        }

        let bundle = BundleInterface::new(bundle_hash, &self.bundle_path_root);
        bundles.insert(bundle_hash.to_string(), bundle.clone());
        bundle
    }

    /// Run a bundle function and return the result as a String.
    /// Mirrors C++ BundleManager::runBundle_string().
    pub unsafe fn run_bundle_string(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> String {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        let _scope = bundle.thread_scope();

        match bundle.run(function_name, details, job_data) {
            Ok(result_obj) => {
                let result = bundle.to_string_py(result_obj);
                bundle.dispose_object(result_obj);
                result
            }
            Err(NoneException) => String::new(),
        }
    }

    /// Run a bundle function and return the result as u64.
    /// Mirrors C++ BundleManager::runBundle_uint64().
    pub unsafe fn run_bundle_uint64(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> u64 {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        let _scope = bundle.thread_scope();

        match bundle.run(function_name, details, job_data) {
            Ok(result_obj) => {
                let result = bundle.to_uint64(result_obj);
                bundle.dispose_object(result_obj);
                result
            }
            Err(NoneException) => 0,
        }
    }

    /// Run a bundle function and return the result as bool.
    /// Mirrors C++ BundleManager::runBundle_bool().
    pub unsafe fn run_bundle_bool(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> bool {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        let _scope = bundle.thread_scope();

        match bundle.run(function_name, details, job_data) {
            Ok(result_obj) => {
                let result = bundle.to_bool(result_obj);
                bundle.dispose_object(result_obj);
                result
            }
            Err(NoneException) => false,
        }
    }

    /// Run a bundle function and return the result as JSON.
    /// Mirrors C++ BundleManager::runBundle_json().
    pub unsafe fn run_bundle_json(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> Value {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        let _scope = bundle.thread_scope();

        match bundle.run(function_name, details, job_data) {
            Ok(result_obj) => {
                let json_str = bundle.json_dumps(result_obj);
                bundle.dispose_object(result_obj);
                serde_json::from_str(&json_str).unwrap_or(Value::Null)
            }
            Err(NoneException) => Value::Null,
        }
    }
}

pub fn get_executable_path() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

pub fn get_default_bundle_path() -> String {
    let mut path = get_executable_path();
    path.pop();
    path.push("bundles");
    path.to_string_lossy().to_string()
}
