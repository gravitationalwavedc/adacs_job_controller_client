mod fixtures;
// Integration tests that require cross-module coordination or complex fixtures
// These tests use shared global state (BundleManager, WebSocket mocks, DB) and must run sequentially
pub mod bundle_db_tests;
mod bundle_logging_tests;
pub mod file_tests;
pub mod job_cancel_tests;
pub mod job_delete_tests;
pub mod job_tests;
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
        crate::python_interface::load_python_library(&crate::config::get_python_library_path());
        unsafe {
            crate::python_interface::PyImport_AppendInittab(
                b"_bundledb\0".as_ptr() as *const std::os::raw::c_char,
                Some(crate::bundle_db::PyInit_bundledb),
            );
            crate::python_interface::PyImport_AppendInittab(
                b"_bundlelogging\0".as_ptr() as *const std::os::raw::c_char,
                Some(crate::bundle_logging::PyInit_bundlelogging),
            );
        }
        crate::python_interface::init_python();
    });
}
