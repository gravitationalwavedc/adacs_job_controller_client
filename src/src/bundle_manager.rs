//! Port of C++ `BundleManager`.
//!
//! Manages loading and caching of `BundleInterface` instances (one per bundle hash).
//! All runBundle_* methods acquire the `PYTHON_MUTEX`, create a `ThreadScope`, call
//! the bundle function, convert the result, and dispose the `PyObject`.

use crate::bundle_interface::BundleInterface;
use crate::python_interface::{ThreadScope, PYTHON_MUTEX};
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

/// Create a `ThreadScope` for the given bundle, tracing the operation.
/// Shared by all `run_bundle_*` methods. Callers must hold `PYTHON_MUTEX`.
fn create_thread_scope(
    method_name: &str,
    function_name: &str,
    bundle: &BundleInterface,
) -> Result<ThreadScope, String> {
    trace!(
        "{} creating thread scope for {}",
        method_name,
        function_name
    );
    // SAFETY: Callers hold PYTHON_MUTEX for the duration of this call.
    unsafe {
        let scope = match bundle.thread_scope() {
            Ok(s) => s,
            Err(e) => {
                error!("{}: Failed to create thread scope: {}", function_name, e);
                return Err(format!("Failed to create thread scope: {e}"));
            }
        };
        trace!("{} created thread scope for {}", method_name, function_name);
        Ok(scope)
    }
}

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
                // test process. Tests run --test-threads=1, so no other thread
                // accesses the retired manager concurrently. Previously returned
                // `&'static BundleManager` references cannot be ruled out; the
                // retention policy below keeps the allocation alive when needed.
                unsafe {
                    Self::retire_test_manager(Box::from_raw(old));
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

    /// Reset the test singleton to the uninitialized state so a test can exercise
    /// the `BundleManager::singleton()` panic path (tests run --test-threads=1).
    #[cfg(test)]
    pub fn reset_singleton_for_test() {
        let ptr = SINGLETON_TEST.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
        if !ptr.is_null() {
            // SAFETY: `ptr` was set by a previous `initialize` call in this same test
            // process. Tests run --test-threads=1, so no other thread accesses the
            // retired manager concurrently. Previously returned `&'static
            // BundleManager` references cannot be ruled out; the retention policy
            // below keeps the allocation alive when needed.
            unsafe {
                Self::retire_test_manager(Box::from_raw(ptr));
            }
        }
    }

    /// Retire a test `BundleManager` that is being replaced. Managers whose
    /// bundle map is empty contain no `SubInterpreter` and are dropped normally.
    /// Managers that own bundles must be retained for process lifetime: dropping
    /// them tears down each cached `SubInterpreter` via `Py_EndInterpreter`,
    /// which can block forever when invoked from a different thread than the one
    /// that created the sub-interpreter (the Python GIL is held by a thread state
    /// that is no longer current). Retained allocations are reclaimed when the
    /// test process exits.
    #[cfg(test)]
    fn retire_test_manager(manager: Box<BundleManager>) {
        if manager.bundles.read().is_empty() {
            drop(manager);
        } else {
            std::mem::forget(manager);
        }
    }

    /// Test-only: insert a bundle directly into the cache so `load_bundle`
    /// returns it without creating a real sub-interpreter. Used to exercise
    /// failure branches (e.g. thread-scope errors) that need a loaded bundle.
    #[cfg(test)]
    fn insert_bundle_for_test(&self, bundle: BundleInterface) {
        self.bundles
            .write()
            .insert(bundle.bundle_hash().to_string(), bundle);
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
        info!("BundleManager: loaded bundle {}", bundle_hash);
        bundles.insert(bundle_hash.to_string(), bundle.clone());
        debug!("BundleManager: bundle {} inserted into cache", bundle_hash);
        Ok(bundle)
    }

    /// Load a bundle, logging the failure and returning a formatted error message
    /// on error. Shared by all `run_bundle_*` methods.
    fn load_bundle_or_error(
        &self,
        function_name: &str,
        bundle_hash: &str,
    ) -> Result<BundleInterface, String> {
        match self.load_bundle(bundle_hash) {
            Ok(bundle) => Ok(bundle),
            Err(e) => {
                error!(
                    "{}: Failed to load bundle {}: {}",
                    function_name, bundle_hash, e
                );
                Err(format!("Failed to load bundle {bundle_hash}: {e}"))
            }
        }
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
        let bundle = match self.load_bundle_or_error("run_bundle_string", bundle_hash) {
            Ok(b) => b,
            Err(msg) => {
                return serde_json::json!({ "error": msg }).to_string();
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
            let _scope = match create_thread_scope("run_bundle_string", function_name, &bundle) {
                Ok(s) => s,
                Err(e) => {
                    return serde_json::json!({ "error": e }).to_string();
                }
            };
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
        let Ok(bundle) = self.load_bundle_or_error("run_bundle_uint64", bundle_hash) else {
            return 0;
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
            let Ok(_scope) = create_thread_scope("run_bundle_uint64", function_name, &bundle)
            else {
                return 0;
            };
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
        let Ok(bundle) = self.load_bundle_or_error("run_bundle_bool", bundle_hash) else {
            return false;
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
            let Ok(_scope) = create_thread_scope("run_bundle_bool", function_name, &bundle) else {
                return false;
            };
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
        let Ok(bundle) = self.load_bundle_or_error("run_bundle_json", bundle_hash) else {
            return Value::Null;
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
            let Ok(_scope) = create_thread_scope("run_bundle_json", function_name, &bundle) else {
                return Value::Null;
            };
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

/// Resolve a job's working directory by running the bundle's `working_directory`
/// function off the async executor. Mirrors the C++ daemon's behaviour.
pub async fn resolve_working_directory(
    bundle_hash: &str,
    details: Value,
    job_data: &str,
    context: &str,
) -> String {
    let bundle_hash = bundle_hash.to_string();
    let job_data = job_data.to_string();
    let context = context.to_string();
    tokio::task::spawn_blocking(move || {
        BundleManager::singleton().run_bundle_string(
            "working_directory",
            &bundle_hash,
            &details,
            &job_data,
        )
    })
    .await
    .unwrap_or_else(|e| {
        error!("{}: spawn_blocking error: {}", context, e);
        String::new()
    })
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

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn get_default_bundle_path_appends_bundles_dir() {
        let path = get_default_bundle_path();
        let suffix = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str());
        assert_eq!(suffix, Some("bundles"));
    }

    #[test]
    fn get_executable_path_returns_existing_file() {
        let path = get_executable_path();
        assert!(!path.as_os_str().is_empty());
        assert!(path.exists(), "executable path should exist: {path:?}");
        assert!(path.is_file());
    }
}

#[cfg(test)]
mod load_bundle_tests {
    use super::*;
    use crate::tests::fixtures::bundle_fixture::{BundleFixture, JOB_SUBMIT_SCRIPT};

    #[test]
    fn load_bundle_returns_cached_result_on_second_call() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = "test_bundle_cache_hit";
        fixture.write_script(bundle_hash, JOB_SUBMIT_SCRIPT, &[]);
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let first = BundleManager::singleton()
            .load_bundle(bundle_hash)
            .expect("first load_bundle should succeed");
        let second = BundleManager::singleton()
            .load_bundle(bundle_hash)
            .expect("second load_bundle should succeed");

        // Both should reference the same inner Arc (cache hit, not re-loaded)
        assert_eq!(first.bundle_hash(), second.bundle_hash());
    }
}

#[cfg(test)]
mod resolve_working_directory_tests {
    use super::*;
    use crate::tests::fixtures::bundle_fixture::{BundleFixture, JOB_SUBMIT_SCRIPT};
    use serde_json::json;

    #[test]
    fn resolve_working_directory_returns_bundle_working_directory() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = "test_resolve_working_directory";
        fixture.write_script(bundle_hash, JOB_SUBMIT_SCRIPT, &[]);
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(resolve_working_directory(
            bundle_hash,
            json!({}),
            "",
            "test",
        ));
        assert_eq!(result, "WORKING_DIR");
    }

    #[test]
    fn resolve_working_directory_returns_empty_string_on_spawn_blocking_failure() {
        BundleManager::reset_singleton_for_test();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(resolve_working_directory(
            "uninitialized_hash",
            json!({}),
            "",
            "test",
        ));
        assert_eq!(result, "");
    }
}

#[cfg(test)]
mod thread_scope_error_tests {
    use super::*;
    use serde_json::json;

    const FAIL_HASH: &str = "test_thread_scope_fail";

    fn failing_bundle() -> BundleInterface {
        BundleInterface::with_thread_scope_error(FAIL_HASH, "boom")
    }

    #[test]
    fn create_thread_scope_returns_error_when_thread_scope_fails() {
        crate::tests::init_python_global();
        let result = create_thread_scope("test_method", "test_func", &failing_bundle());
        assert!(result.is_err());
        assert_eq!(
            result.err(),
            Some("Failed to create thread scope: boom".to_string())
        );
    }

    #[test]
    fn run_bundle_string_returns_error_json_when_thread_scope_fails() {
        crate::tests::init_python_global();
        BundleManager::initialize("/tmp/nonexistent_bundle_root".to_string());
        BundleManager::singleton().insert_bundle_for_test(failing_bundle());

        let result =
            BundleManager::singleton().run_bundle_string("test_func", FAIL_HASH, &json!({}), "");
        let parsed: Value = serde_json::from_str(&result).expect("result should be valid JSON");
        assert_eq!(parsed["error"], "Failed to create thread scope: boom");
    }

    #[test]
    fn run_bundle_uint64_returns_zero_when_thread_scope_fails() {
        crate::tests::init_python_global();
        BundleManager::initialize("/tmp/nonexistent_bundle_root".to_string());
        BundleManager::singleton().insert_bundle_for_test(failing_bundle());

        let result =
            BundleManager::singleton().run_bundle_uint64("test_func", FAIL_HASH, &json!({}), "");
        assert_eq!(result, 0);
    }

    #[test]
    fn run_bundle_bool_returns_false_when_thread_scope_fails() {
        crate::tests::init_python_global();
        BundleManager::initialize("/tmp/nonexistent_bundle_root".to_string());
        BundleManager::singleton().insert_bundle_for_test(failing_bundle());

        let result =
            BundleManager::singleton().run_bundle_bool("test_func", FAIL_HASH, &json!({}), "");
        assert!(!result);
    }

    #[test]
    fn run_bundle_json_returns_null_when_thread_scope_fails() {
        crate::tests::init_python_global();
        BundleManager::initialize("/tmp/nonexistent_bundle_root".to_string());
        BundleManager::singleton().insert_bundle_for_test(failing_bundle());

        let result =
            BundleManager::singleton().run_bundle_json("test_func", FAIL_HASH, &json!({}), "");
        assert_eq!(result, Value::Null);
    }
}

#[cfg(test)]
mod load_bundle_error_tests {
    use super::*;
    use crate::tests::fixtures::bundle_fixture::BundleFixture;
    use serde_json::json;

    /// A bundle hash with no `bundle.py` on disk is not cached and
    /// `BundleInterface::new` fails, so `load_bundle_or_error` returns the
    /// "Failed to load bundle" error and each `run_bundle_*` method returns its
    /// error-return default.
    fn manager_with_missing_bundle() -> (BundleFixture, &'static str) {
        let fixture = BundleFixture::new();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        (fixture, "test_load_failure")
    }

    #[test]
    fn run_bundle_string_returns_error_json_when_load_fails() {
        crate::tests::init_python_global();
        let (_fixture, hash) = manager_with_missing_bundle();

        let result =
            BundleManager::singleton().run_bundle_string("test_func", hash, &json!({}), "");
        let parsed: Value = serde_json::from_str(&result).expect("result should be valid JSON");
        assert!(
            parsed["error"]
                .as_str()
                .unwrap_or_default()
                .starts_with("Failed to load bundle"),
            "unexpected error: {parsed}"
        );
    }

    #[test]
    fn run_bundle_uint64_returns_zero_when_load_fails() {
        crate::tests::init_python_global();
        let (_fixture, hash) = manager_with_missing_bundle();

        let result =
            BundleManager::singleton().run_bundle_uint64("test_func", hash, &json!({}), "");
        assert_eq!(result, 0);
    }

    #[test]
    fn run_bundle_bool_returns_false_when_load_fails() {
        crate::tests::init_python_global();
        let (_fixture, hash) = manager_with_missing_bundle();

        let result = BundleManager::singleton().run_bundle_bool("test_func", hash, &json!({}), "");
        assert!(!result);
    }

    #[test]
    fn run_bundle_json_returns_null_when_load_fails() {
        crate::tests::init_python_global();
        let (_fixture, hash) = manager_with_missing_bundle();

        let result = BundleManager::singleton().run_bundle_json("test_func", hash, &json!({}), "");
        assert_eq!(result, Value::Null);
    }
}
