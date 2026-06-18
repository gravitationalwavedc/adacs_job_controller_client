//! Port of C++ `BundleManager`.
//!
//! Manages loading and caching of `BundleInterface` instances (one per bundle hash).
//! All runBundle_* methods acquire the `PYTHON_MUTEX`, create a `ThreadScope`, call
//! the bundle function, convert the result, and dispose the `PyObject`.

use crate::bundle_interface::BundleInterface;
use crate::python_interface::PYTHON_MUTEX;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, error, info, trace};

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
    pub fn load_bundle(&self, bundle_hash: &str) -> Result<BundleInterface, String> {
        debug!(
            "BundleManager: load_bundle() called for hash '{}'",
            bundle_hash
        );
        // Check if already loaded (read lock)
        {
            let bundles = self.bundles.read();
            if let Some(bundle) = bundles.get(bundle_hash) {
                debug!("BundleManager: using cached bundle {}", bundle_hash);
                return Ok(bundle.clone());
            }
        }

        // Not loaded – acquire write lock and create
        debug!(
            "BundleManager: bundle {} not cached, acquiring write lock",
            bundle_hash
        );
        let lock_start = std::time::Instant::now();
        let mut bundles = self.bundles.write();
        trace!(
            "BundleManager: acquired write lock in {:?}",
            lock_start.elapsed()
        );
        // Double-check after acquiring write lock
        if let Some(existing) = bundles.get(bundle_hash) {
            debug!(
                "BundleManager: bundle {} loaded by another thread, using cached",
                bundle_hash
            );
            return Ok(existing.clone());
        }

        // SAFETY: BundleInterface::new requires that Python has been initialised
        // (init_python was called) and that the bundle path is valid.
        info!(
            "BundleManager: loading bundle {} from {}",
            bundle_hash, self.bundle_path_root
        );
        let bundle = unsafe { BundleInterface::new(bundle_hash, &self.bundle_path_root)? };
        info!(
            "BundleManager: loaded bundle {} ({} bytes)",
            bundle_hash,
            bundle_hash.len()
        );
        bundles.insert(bundle_hash.to_string(), bundle.clone());
        debug!("BundleManager: bundle {} inserted into cache", bundle_hash);
        Ok(bundle)
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
        debug!(
            "run_bundle_string entering {} for bundle {} with details={}",
            function_name, bundle_hash, details
        );
        let bundle = match self.load_bundle(bundle_hash) {
            Ok(b) => b,
            Err(e) => {
                error!(
                    "run_bundle_string: Failed to load bundle {}: {}",
                    bundle_hash, e
                );
                return serde_json::json!({
                    "error": format!("Failed to load bundle {}: {}", bundle_hash, e)
                })
                .to_string();
            }
        };
        debug!(
            "run_bundle_string loaded bundle {} for {}",
            bundle_hash, function_name
        );

        let mutex_start = std::time::Instant::now();
        let _guard = PYTHON_MUTEX.lock();
        let mutex_time = mutex_start.elapsed();
        trace!(
            "run_bundle_string: acquired PYTHON_MUTEX in {:?}",
            mutex_time
        );
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
            trace!(
                "run_bundle_string creating thread scope for {}",
                function_name
            );
            let _scope = match bundle.thread_scope() {
                Ok(s) => s,
                Err(e) => {
                    error!("{}: Failed to create thread scope: {}", function_name, e);
                    return serde_json::json!({
                        "error": format!("Failed to create thread scope: {}", e)
                    })
                    .to_string();
                }
            };
            trace!(
                "run_bundle_string created thread scope for {}",
                function_name
            );
            if let Ok(result_obj) = bundle.run(function_name, details, job_data) {
                trace!(
                    "run_bundle_string bundle.run returned for {}",
                    function_name
                );
                let result = bundle.to_string_py(result_obj);
                bundle.dispose_object(result_obj);
                debug!(
                    "run_bundle_string completed {} - result len={}",
                    function_name,
                    result.len()
                );
                result
            } else {
                debug!(
                    "run_bundle_string completed {} - returned None",
                    function_name
                );
                String::new()
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
        debug!(
            "run_bundle_uint64 entering {} for bundle {} with details={}",
            function_name, bundle_hash, details
        );
        let bundle = match self.load_bundle(bundle_hash) {
            Ok(b) => b,
            Err(e) => {
                error!(
                    "run_bundle_uint64: Failed to load bundle {}: {}",
                    bundle_hash, e
                );
                return 0;
            }
        };
        debug!(
            "run_bundle_uint64 loaded bundle {} for {}",
            bundle_hash, function_name
        );

        let mutex_start = std::time::Instant::now();
        let _guard = PYTHON_MUTEX.lock();
        let mutex_time = mutex_start.elapsed();
        trace!(
            "run_bundle_uint64: acquired PYTHON_MUTEX in {:?}",
            mutex_time
        );
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
            trace!(
                "run_bundle_uint64 creating thread scope for {}",
                function_name
            );
            let _scope = match bundle.thread_scope() {
                Ok(s) => s,
                Err(e) => {
                    error!("{}: Failed to create thread scope: {}", function_name, e);
                    return 0;
                }
            };
            trace!(
                "run_bundle_uint64 created thread scope for {}",
                function_name
            );
            if let Ok(result_obj) = bundle.run(function_name, details, job_data) {
                trace!(
                    "run_bundle_uint64 bundle.run returned for {}",
                    function_name
                );
                let result = bundle.to_uint64(result_obj);
                bundle.dispose_object(result_obj);
                debug!(
                    "run_bundle_uint64 completed {} - result={}",
                    function_name, result
                );
                result
            } else {
                debug!(
                    "run_bundle_uint64 completed {} - returned 0 (None)",
                    function_name
                );
                0
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
        debug!(
            "run_bundle_bool entering {} for bundle {} with details={}",
            function_name, bundle_hash, details
        );
        let bundle = match self.load_bundle(bundle_hash) {
            Ok(b) => b,
            Err(e) => {
                error!(
                    "run_bundle_bool: Failed to load bundle {}: {}",
                    bundle_hash, e
                );
                return false;
            }
        };
        debug!(
            "run_bundle_bool loaded bundle {} for {}",
            bundle_hash, function_name
        );

        let mutex_start = std::time::Instant::now();
        let _guard = PYTHON_MUTEX.lock();
        let mutex_time = mutex_start.elapsed();
        trace!("run_bundle_bool: acquired PYTHON_MUTEX in {:?}", mutex_time);
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
            trace!(
                "run_bundle_bool creating thread scope for {}",
                function_name
            );
            let _scope = match bundle.thread_scope() {
                Ok(s) => s,
                Err(e) => {
                    error!("{}: Failed to create thread scope: {}", function_name, e);
                    return false;
                }
            };
            trace!("run_bundle_bool created thread scope for {}", function_name);
            if let Ok(result_obj) = bundle.run(function_name, details, job_data) {
                trace!("run_bundle_bool bundle.run returned for {}", function_name);
                let result = bundle.to_bool(result_obj);
                bundle.dispose_object(result_obj);
                debug!(
                    "run_bundle_bool completed {} - result={}",
                    function_name, result
                );
                result
            } else {
                debug!(
                    "run_bundle_bool completed {} - returned false (None)",
                    function_name
                );
                false
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
        debug!(
            "run_bundle_json entering {} for bundle {} with details={}",
            function_name, bundle_hash, details
        );
        let bundle = match self.load_bundle(bundle_hash) {
            Ok(b) => b,
            Err(e) => {
                error!(
                    "run_bundle_json: Failed to load bundle {}: {}",
                    bundle_hash, e
                );
                return Value::Null;
            }
        };
        debug!(
            "run_bundle_json loaded bundle {} for {}",
            bundle_hash, function_name
        );

        let mutex_start = std::time::Instant::now();
        let _guard = PYTHON_MUTEX.lock();
        let mutex_time = mutex_start.elapsed();
        trace!("run_bundle_json: acquired PYTHON_MUTEX in {:?}", mutex_time);
        // SAFETY: PYTHON_MUTEX is held above for the duration of this block.
        unsafe {
            trace!(
                "run_bundle_json creating thread scope for {}",
                function_name
            );
            let _scope = match bundle.thread_scope() {
                Ok(s) => s,
                Err(e) => {
                    error!("{}: Failed to create thread scope: {}", function_name, e);
                    return Value::Null;
                }
            };
            trace!("run_bundle_json created thread scope for {}", function_name);
            if let Ok(result_obj) = bundle.run(function_name, details, job_data) {
                trace!("run_bundle_json bundle.run returned for {}", function_name);
                let json_str = match bundle.json_dumps(result_obj) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("run_bundle_json: Failed to serialize result: {}", e);
                        serde_json::json!({
                            "error": format!("Failed to serialize result: {}", e)
                        })
                        .to_string()
                    }
                };
                bundle.dispose_object(result_obj);
                let result = serde_json::from_str(&json_str).unwrap_or(Value::Null);
                debug!(
                    "run_bundle_json completed {} - result={}",
                    function_name, result
                );
                result
            } else {
                debug!(
                    "run_bundle_json completed {} - returned Null (None)",
                    function_name
                );
                Value::Null
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
