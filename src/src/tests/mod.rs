pub mod fixtures;
// Integration tests that require cross-module coordination or complex fixtures
// These tests use shared global state (BundleManager, WebSocket mocks, DB) and must run sequentially
pub mod bundle_db_tests;
mod bundle_logging_tests;
pub mod file_tests;
pub mod job_cancel_tests;
pub mod job_delete_tests;
pub mod job_tests;
pub mod main_tests;
// Note: Unit tests for individual modules are now in their respective source files:
// - messaging.rs has messaging/serialization tests
// - daemon.rs has daemon fork tests
// - websocket.rs has WebSocket client and auth tests
// - jobs/mod.rs has job handler tests
// - files/mod.rs has file handler tests (if any)
// - db/mod.rs has database tests (if any)

// Global Python initialization - runs once before any tests
#[cfg(test)]
static INIT_PYTHON: std::sync::Once = std::sync::Once::new();

#[cfg(test)]
fn init_python_global() {
    INIT_PYTHON.call_once(|| {
        // Default TEST_CONFIG for all tests. Individual tests may override
        // fields (e.g. file_tests::set_test_config sets a real
        // websocketEndpoint port). This must be set in every test process,
        // including test-fork children, because `test-fork-core` re-execs
        // the test binary which starts with an empty TEST_CONFIG and would
        // otherwise fall through to reading a non-existent config.json from
        // next to the test executable. Seed the bare minimum first, then
        // upgrade with the resolved python library path below.
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(serde_json::json!({
            "cluster": "test_cluster",
            "websocketEndpoint": "ws://127.0.0.1:0/ws/",
            "ltk": "test_token",
        }));
        let python_library = crate::config::get_python_library_path();
        *crate::config::TEST_CONFIG.lock().unwrap() = Some(serde_json::json!({
            "cluster": "test_cluster",
            "pythonLibrary": python_library,
            "websocketEndpoint": "ws://127.0.0.1:0/ws/",
            "ltk": "test_token",
        }));
        crate::python_interface::load_python_library(&crate::config::get_python_library_path());
        unsafe {
            crate::python_interface::PyImport_AppendInittab(
                c"_bundledb".as_ptr(),
                Some(crate::bundle_db::PyInit_bundledb),
            );
            crate::python_interface::PyImport_AppendInittab(
                c"_bundlelogging".as_ptr(),
                Some(crate::bundle_logging::PyInit_bundlelogging),
            );
        }
        crate::python_interface::init_python();
    });
}
