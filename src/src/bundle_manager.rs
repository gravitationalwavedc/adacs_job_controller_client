//! Port of C++ `BundleManager`.
//!
//! Manages loading and caching of `BundleInterface` instances (one per bundle hash).
//! All runBundle_* methods acquire the `PYTHON_MUTEX`, create a `ThreadScope`, call
//! the bundle function, convert the result, and dispose the `PyObject`.

use crate::bundle_interface::{BundleInterface, NoneException};
use crate::python_interface::PYTHON_MUTEX;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct BundleManager {
    bundle_path_root: String,
    bundles: RwLock<HashMap<String, BundleInterface>>,
}

// Production: OnceLock - set once, never replaced.
#[cfg(not(test))]
static SINGLETON: std::sync::OnceLock<BundleManager> = std::sync::OnceLock::new();

// Tests: AtomicPtr - resettable between tests (tests run --test-threads=1 so
// replacement is sequential).
#[cfg(test)]
static SINGLETON_TEST: std::sync::atomic::AtomicPtr<BundleManager> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

impl BundleManager {
    pub fn initialize(bundle_path_root: String) {
        let manager = BundleManager {
            bundle_path_root,
            bundles: RwLock::new(HashMap::new()),
        };

        #[cfg(not(test))]
        {
            // Ignore if already initialized (idempotent in production).
            let _ = SINGLETON.set(manager);
        }

        #[cfg(test)]
        {
            let ptr = Box::into_raw(Box::new(manager));
            let old = SINGLETON_TEST.swap(ptr, std::sync::atomic::Ordering::SeqCst);
            if !old.is_null() {
                // SAFETY: `old` was set by a previous `initialize` call in this same
                // test process and is no longer accessible to any other thread (tests
                // run --test-threads=1).
                unsafe {
                    drop(Box::from_raw(old));
                }
            }
        }
    }

    pub fn singleton() -> &'static BundleManager {
        #[cfg(not(test))]
        return SINGLETON.get().expect("BundleManager not initialized");

        #[cfg(test)]
        {
            let ptr = SINGLETON_TEST.load(std::sync::atomic::Ordering::SeqCst);
            assert!(!ptr.is_null(), "BundleManager not initialized");
            // SAFETY: ptr is set by `initialize` and valid for the lifetime of the test.
            unsafe { &*ptr }
        }
    }

    /// Load (or return cached) `BundleInterface` for a given hash.
    /// Mirrors C++ `BundleManager::loadBundle()`.
    pub fn load_bundle(&self, bundle_hash: &str) -> BundleInterface {
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

        // SAFETY: BundleInterface::new requires that Python has been initialised
        // (init_python was called) and that the bundle path is valid.
        let bundle = unsafe { BundleInterface::new(bundle_hash, &self.bundle_path_root) };
        bundles.insert(bundle_hash.to_string(), bundle.clone());
        bundle
    }

    /// Run a bundle function and return the result as a String.
    /// Mirrors C++ `BundleManager::runBundle_string()`.
    pub fn run_bundle_string(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> String {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
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
    }

    /// Run a bundle function and return the result as u64.
    /// Mirrors C++ `BundleManager::runBundle_uint64()`.
    pub fn run_bundle_uint64(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> u64 {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
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
    }

    /// Run a bundle function and return the result as bool.
    /// Mirrors C++ `BundleManager::runBundle_bool()`.
    pub fn run_bundle_bool(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> bool {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
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
    }

    /// Run a bundle function and return the result as JSON.
    /// Mirrors C++ `BundleManager::runBundle_json()`.
    pub fn run_bundle_json(
        &self,
        function_name: &str,
        bundle_hash: &str,
        details: &Value,
        job_data: &str,
    ) -> Value {
        let bundle = self.load_bundle(bundle_hash);

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
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
