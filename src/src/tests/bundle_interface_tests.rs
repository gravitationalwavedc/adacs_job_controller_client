//! Regression tests for `BundleInterface::print_last_python_exception`.
//!
//! See glab issue #4. The production symptom is that the exception printer
//! loses traceback frames when Python's high-level traceback formatting path
//! fails. These tests lock in the required fallback behavior: preserve and
//! print traceback frames, then emit a synthesized final exception line.
//!
//! These tests intentionally raise uncaught Python exceptions from
//! bundle scripts and capture the structured log output to verify that
//! the full stack trace is printed to the console.

use crate::bundle_interface::BundleInterface;
use crate::bundle_manager::BundleManager;
use crate::messaging::{Message, Priority, DB_RESPONSE};
use crate::python_interface::{
    my_py_none_struct, set_py_run_string_flags_override, set_py_tuple_set_item_override,
    PyDict_GetItemString, PyDict_New, PyDict_SetItemString, PyErr_Occurred, PyErr_SetString,
    PyEval_GetBuiltins, PyImport_ImportModule, PyLong_FromUnsignedLongLong, PyObject,
    PyObject_SetAttrString, PyRun_StringFlags, PyTupleSetItemFn, PyTuple_SetItem, PyTuple_Size,
    PyUnicode_FromString, Py_DecRef, Py_IncRef, Py_file_input, Py_ssize_t, PYTHON_MUTEX,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use std::ffi::CString;
use std::io::Write;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Arc, Mutex};
use test_fork::test;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

#[derive(Clone, Default)]
struct VecWriter(Arc<Mutex<Vec<u8>>>);

impl VecWriter {
    fn new() -> Self {
        Self::default()
    }

    fn into_string(self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap_or_default()
    }
}

impl<'a> MakeWriter<'a> for VecWriter {
    type Writer = VecWriterGuard;

    fn make_writer(&'a self) -> Self::Writer {
        VecWriterGuard(self.0.clone())
    }
}

struct VecWriterGuard(Arc<Mutex<Vec<u8>>>);

impl Write for VecWriterGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn make_db_response() -> Message {
    Message::new(DB_RESPONSE, Priority::Medium, "database")
}

/// Run `f` with a thread-local tracing subscriber that captures all
/// `INFO`-and-above events into a `String`. Returns the captured log.
fn capture_logs<F: FnOnce()>(f: F) -> String {
    let writer = VecWriter::new();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    writer.into_string()
}

/// REGRESSION TEST (issue #4 — original bug scenario).
///
/// Triggers an uncaught `_bundledb.error` from a bundle script. The mock WS
/// returns `count = 0` so `_bundledb.get_job_by_id(9999)` raises an uncaught
/// exception.
///
/// Verifies that we print traceback frames and a clean final exception line
/// instead of dropping the traceback.
#[test]
fn test_print_last_python_exception_uncaught_bundledb_error_prints_traceback() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Bundle script intentionally does NOT catch the _bundledb error.
        let script = r#"
import _bundledb

def submit(details, job_data):
    _bundledb.get_job_by_id(9999)
    return {"ok": True}
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_uint(0); // count = 0 → "Job with ID 9999 does not exist"
            Box::pin(async move { Ok(resp) })
        });
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            // The Python function raises; BundleInterface::run() catches
            // the error and calls print_last_python_exception() before
            // returning Err(NoneException). run_bundle_json maps that to
            // Value::Null — we don't care about the return, only the logs.
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // 1. The Python exception header line is always present (both
        //    old and new code log it). Useful as a sanity check that
        //    the test reached the exception-printing path at all.
        assert!(
            logs.contains("Python exception: type="),
            "expected 'Python exception: type=' in logs, got:\n{logs}"
        );

        // 2. The new "Traceback (most recent call last):" header must
        //    be present — this is the signature of the FIXED impl.
        //    The OLD code never emits this line because the
        //    format_exception call fails and the function returns
        //    before reaching the log-iteration code.
        assert!(
            logs.contains("Traceback (most recent call last):"),
            "expected 'Traceback (most recent call last):' header in logs, got:\n{logs}"
        );

        // 3. At least one real Python frame in the traceback.
        let frame_count = logs
            .lines()
            .filter(|l| l.contains("File \"") && l.contains(", line "))
            .count();
        assert!(
            frame_count >= 1,
            "expected at least one 'File \"...\", line N' traceback frame, got:\n{logs}"
        );

        // 4. The exception message must be present.
        assert!(
            logs.contains("does not exist"),
            "expected 'does not exist' (from 'Job with ID 9999 does not exist.') in logs, got:\n{logs}"
        );

        assert!(
            logs.contains("error: Job with ID 9999 does not exist."),
            "expected synthesized final exception line without repr quotes, got:\n{logs}"
        );

        // 5. The OLD-code "Error printing active python exception"
        //    marker must NOT appear — its presence means the fix is
        //    not in effect.
        assert!(
            !logs.contains(
                "Error printing active python exception with traceback.format_exception"
            ),
            "old-code 'Error printing active python exception' marker still present — fix not effective:\n{logs}"
        );
    }
    inner();
}

/// REGRESSION TEST (user-mandated scenario from glab issue #4: "create a
/// test that intentionally raises an uncaught exception that we print
/// to console including full stack trace").
///
/// Bundle script raises a plain `RuntimeError` from a 3-level Python
/// call chain. No DB calls are made. Verifies that the captured log
/// contains:
///   1. The `"Traceback (most recent call last):"` header.
///   2. At least 3 `"File \"...\", line N"` frames — one per level.
///   3. The exception class name and message (`RuntimeError: ...`).
///   4. The names of all three Python functions in the call chain.
#[test]
fn test_print_last_python_exception_uncaught_runtime_error_prints_full_stack() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // 3-level call chain raising an uncaught RuntimeError.
        let script = r#"
def level_3():
    raise RuntimeError("intentional failure for regression test")

def level_2():
    level_3()

def level_1():
    level_2()

def submit(details, job_data):
    level_1()
    return {"ok": True}
"#;
        fixture.write_raw_script(&bundle_hash, script);

        // No DB calls expected — the error fires before any DB request.
        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // 1. Traceback header.
        assert!(
            logs.contains("Traceback (most recent call last):"),
            "expected 'Traceback (most recent call last):' in logs, got:\n{logs}"
        );

        // 2. At least 3 frame lines.
        let frame_lines: Vec<&str> = logs
            .lines()
            .filter(|l| l.contains("File \"") && l.contains(", line "))
            .collect();
        assert!(
            frame_lines.len() >= 3,
            "expected at least 3 'File ... line' traceback frames, got {}:\n{logs}",
            frame_lines.len()
        );

        // 3. Exception class + message.
        assert!(
            logs.contains("RuntimeError: intentional failure for regression test"),
            "expected 'RuntimeError: intentional failure for regression test' in logs, got:\n{logs}"
        );

        // 4. All three Python function names should appear in frames.
        for func in ["level_3", "level_2", "level_1"] {
            assert!(
                logs.contains(func),
                "expected function name '{func}' in traceback frames, got:\n{logs}"
            );
        }
    }
    inner();
}

/// REGRESSION TEST (behavioral fallback guard).
///
/// The production symptom is not merely "raw-string value exists"; it is
/// "the traceback frames are dropped when the high-level formatter fails".
///
/// This test forces that formatter failure deterministically by monkey-
/// patching the bundle interpreter's `traceback.format_exception` function
/// to always raise. The bundle then raises a normal uncaught exception.
///
/// OLD implementation: a single `format_exception(...)` call fails and the
/// printer returns early, so the traceback frames are lost.
///
/// NEW implementation: frame formatting uses `format_tb(tb)` separately, so
/// even if the combined formatter path is broken, the traceback frames are
/// still printed.
#[test]
fn test_print_last_python_exception_keeps_traceback_when_format_exception_is_broken() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Monkey-patch traceback.format_exception to fail, then raise a
        // normal uncaught exception from a nested call stack.
        let script = r#"
import traceback

def broken_format_exception(*args, **kwargs):
    raise RuntimeError("forced format_exception failure for regression test")

traceback.format_exception = broken_format_exception

def level_2():
    raise RuntimeError("nested failure for fallback regression test")

def level_1():
    level_2()

def submit(details, job_data):
    level_1()
    return {"ok": True}
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // 1. The exception header line is always present.
        assert!(
            logs.contains("Python exception: type="),
            "expected 'Python exception: type=' in logs, got:\n{logs}"
        );

        // 2. The OLD-code "Error printing active python exception"
        //    marker must NOT appear — the fix must have suppressed it.
        assert!(
            !logs.contains(
                "Error printing active python exception with traceback.format_exception"
            ),
            "old-code 'Error printing active python exception' marker still present — fix not effective:\n{logs}"
        );

        // 3. The real regression target: traceback frames must survive
        //    even though the legacy formatter path is broken.
        assert!(
            logs.contains("Traceback (most recent call last):"),
            "expected traceback header in logs, got:\n{logs}"
        );

        let frame_count = logs
            .lines()
            .filter(|l| l.contains("File \"") && l.contains(", line "))
            .count();
        assert!(
            frame_count >= 1,
            "expected at least one traceback frame in logs, got:\n{logs}"
        );

        assert!(
            logs.contains("level_1"),
            "expected level_1 frame in logs, got:\n{logs}"
        );
        assert!(
            logs.contains("level_2"),
            "expected level_2 frame in logs, got:\n{logs}"
        );

        // 4. The final exception line must still include the class name.
        assert!(
            logs.contains("RuntimeError: nested failure for fallback regression test"),
            "expected final exception line in logs, got:\n{logs}"
        );

        assert!(
            !logs
                .contains("Error printing active python exception with traceback.format_exception"),
            "did not expect old formatter failure marker, got:\n{logs}"
        );
    }
    inner();
}

/// REGRESSION TEST (reference-leak fix in `print_last_python_exception`).
///
/// Covers the branch where `PyObject_GetAttrString(traceback_module,
/// "format_tb")` returns NULL (here forced by deleting `traceback.format_tb`
/// in the bundle). The fix releases the `traceback` ref that `PyErr_Fetch`
/// returned, since `tb_args` was never created to steal it. Behaviorally the
/// printer must still emit the exception header via `format_exception_only`
/// and must NOT emit traceback frames (the formatter is unavailable).
#[test]
fn test_print_last_python_exception_releases_traceback_when_format_tb_missing() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Delete traceback.format_tb so the attribute lookup inside
        // print_last_python_exception fails and hits the is_null() branch.
        let script = r#"
import traceback
del traceback.format_tb

def submit(details, job_data):
    raise RuntimeError("format_tb missing for leak regression test")
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // 1. The exception printer was reached.
        assert!(
            logs.contains("Python exception: type="),
            "expected 'Python exception: type=' in logs, got:\n{logs}"
        );

        // 2. format_tb was unavailable, so no traceback frames are printed.
        assert!(
            !logs.contains("Traceback (most recent call last):"),
            "did not expect traceback header when traceback.format_tb is missing, got:\n{logs}"
        );

        // 3. The final exception header still comes through via
        //    format_exception_only (which is untouched by this test).
        assert!(
            logs.contains("RuntimeError: format_tb missing for leak regression test"),
            "expected final exception header in logs, got:\n{logs}"
        );
    }
    inner();
}

/// REGRESSION TEST (reference-leak fix in `print_last_python_exception`).
///
/// Covers the branch where `PyObject_GetAttrString(traceback_module,
/// "format_exception_only")` returns NULL (here forced by deleting
/// `traceback.format_exception_only` in the bundle). The fix releases the
/// `extype` and `value` refs that `PyErr_Fetch` returned, since `eo_args`
/// was never created to steal them. Behaviorally the printer must still
/// emit the synthesized `type: value` fallback line.
#[test]
fn test_print_last_python_exception_releases_exception_when_format_exception_only_missing() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Delete traceback.format_exception_only so the attribute lookup
        // inside print_last_python_exception fails and hits the is_null()
        // branch.
        let script = r#"
import traceback
del traceback.format_exception_only

def submit(details, job_data):
    raise RuntimeError("format_exception_only missing for leak regression test")
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // 1. The exception printer was reached.
        assert!(
            logs.contains("Python exception: type="),
            "expected 'Python exception: type=' in logs, got:\n{logs}"
        );

        // 2. format_tb is untouched, so the traceback frames are printed.
        assert!(
            logs.contains("Traceback (most recent call last):"),
            "expected traceback header in logs, got:\n{logs}"
        );

        // 3. format_exception_only is unavailable, so the synthesized
        //    fallback line must be emitted instead.
        assert!(
            logs.contains("RuntimeError: format_exception_only missing for leak regression test"),
            "expected synthesized fallback exception line in logs, got:\n{logs}"
        );
    }
    inner();
}

#[test]
fn test_bundle_load_failure_no_panic() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let bundle_hash = "non_existent_bundle_hash_12345";

        // Initialize BundleManager with a dummy path
        BundleManager::initialize("/tmp/non_existent_path_67890".to_string());

        // Calling run_bundle_json should return Value::Null and NOT panic
        let result_json = BundleManager::singleton().run_bundle_json(
            "submit",
            bundle_hash,
            &serde_json::json!({}),
            "",
        );
        assert_eq!(result_json, serde_json::Value::Null);

        // Calling run_bundle_string should return a serialized json error and NOT panic
        let result_str = BundleManager::singleton().run_bundle_string(
            "submit",
            bundle_hash,
            &serde_json::json!({}),
            "",
        );
        assert!(
            result_str.contains("Failed to load bundle"),
            "result_str: {result_str}"
        );

        // Calling run_bundle_uint64 should return 0 and NOT panic
        let result_u64 = BundleManager::singleton().run_bundle_uint64(
            "submit",
            bundle_hash,
            &serde_json::json!({}),
            "",
        );
        assert_eq!(result_u64, 0);

        // Calling run_bundle_bool should return false and NOT panic
        let result_bool = BundleManager::singleton().run_bundle_bool(
            "submit",
            bundle_hash,
            &serde_json::json!({}),
            "",
        );
        assert!(!result_bool);
    }
    inner();
}

/// DIRECT UNIT TEST for `BundleInterface::run` — reviewer request on
/// MR !189 ("Coverage?").
///
/// Success path: a bundle function that returns a dict round-trips through
/// `run` and `json_dumps` unchanged.
#[test]
fn test_run_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {\"ok\": True, \"job_data\": job_data}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let result = bundle
                .run("submit", &serde_json::json!({"a": 1}), "job-data")
                .unwrap_or_else(|_| panic!("run should succeed"));
            assert!(!result.is_null(), "run should return a non-null PyObject");
            let dumped = bundle
                .json_dumps(result)
                .expect("json_dumps should succeed");
            BundleInterface::dispose_object(result);
            let parsed: serde_json::Value = serde_json::from_str(&dumped).unwrap();
            assert_eq!(parsed["ok"], true);
            assert_eq!(parsed["job_data"], "job-data");
        }
    }
    inner();
}

/// A NUL byte in `func` makes `CString::new` fail, so `run` must return
/// `Err(NoneException)` instead of panicking.
#[test]
fn test_run_returns_err_for_nul_byte_func() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let result = bundle.run("sub\0mit", &serde_json::json!({}), "");
            assert!(
                result.is_err(),
                "NUL byte in func should make run return Err"
            );
        }
    }
    inner();
}

/// A NUL byte in `job_data` makes `CString::new` fail, so `run` must return
/// `Err(NoneException)` instead of panicking. This is the branch immediately
/// before the new `PyUnicode_FromString` null guard.
#[test]
fn test_run_returns_err_for_nul_byte_job_data() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let result = bundle.run("submit", &serde_json::json!({}), "job\0data");
            assert!(
                result.is_err(),
                "NUL byte in job_data should make run return Err"
            );
        }
    }
    inner();
}

/// A missing bundle function makes `PyObject_GetAttrString` return NULL, so
/// `run` must return `Err(NoneException)`.
#[test]
fn test_run_returns_err_for_missing_function() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let result = bundle.run("does_not_exist", &serde_json::json!({}), "");
            assert!(
                result.is_err(),
                "missing function should make run return Err"
            );
        }
    }
    inner();
}

/// A bundle attribute that exists but is not callable (e.g. `submit = 42`)
/// makes `PyCallable_Check` return 0, so `run` must return
/// `Err(NoneException)`.
#[test]
fn test_run_returns_err_for_non_callable_function() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(&bundle_hash, "submit = 42\n");

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let result = bundle.run("submit", &serde_json::json!({}), "");
            assert!(
                result.is_err(),
                "non-callable function should make run return Err"
            );
        }
    }
    inner();
}

/// A bundle function that returns `None` makes `run` return
/// `Err(NoneException)`.
#[test]
fn test_run_returns_err_when_function_returns_none() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return None\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let result = bundle.run("submit", &serde_json::json!({}), "");
            assert!(
                result.is_err(),
                "function returning None should make run return Err"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for `BundleInterface::json_loads` — reviewer request on
/// MR !186 ("cover the rest of this function/verify correctness?").
///
/// Success path: valid JSON parses to a Python object that round-trips
/// through `json_dumps` unchanged.
#[test]
fn test_json_loads_parses_valid_json() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let obj = bundle.json_loads(r#"{"key": "value", "n": 42}"#);
            assert!(
                !obj.is_null(),
                "valid JSON should parse to a non-null PyObject"
            );
            let dumped = bundle.json_dumps(obj).expect("json_dumps should succeed");
            BundleInterface::dispose_object(obj);
            let parsed: serde_json::Value = serde_json::from_str(&dumped).unwrap();
            assert_eq!(parsed["key"], "value");
            assert_eq!(parsed["n"], 42);
        }
    }
    inner();
}

/// NUL-byte content: `CString::new` fails, so `json_loads` must return NULL
/// (the "content contains NUL byte" branch) instead of panicking.
#[test]
fn test_json_loads_returns_null_for_nul_byte_content() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let obj = bundle.json_loads("abc\0def");
            assert!(obj.is_null(), "content with NUL byte should return NULL");
        }
    }
    inner();
}

/// Invalid JSON: `json.loads` raises a Python exception, so `json_loads`
/// must return NULL (the "Error calling json.loads" branch).
#[test]
fn test_json_loads_returns_null_for_invalid_json() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let obj = bundle.json_loads("this is not valid json");
            assert!(obj.is_null(), "invalid JSON should return NULL");
        }
    }
    inner();
}

/// The `json.loads` attribute lookup can fail (e.g. when the `loads`
/// attribute is removed from the json module), so `json_loads` must return
/// NULL (the "failed to get json.loads function" branch) instead of
/// dereferencing a null function pointer.
#[test]
fn test_json_loads_returns_null_when_loads_lookup_fails() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            // The json module is cached in the sub-interpreter, so this
            // returns the same module object `json_loads` reads from.
            let json_module = PyImport_ImportModule(c"json".as_ptr());
            assert!(!json_module.is_null(), "json module should import");
            let c_name = CString::new("loads").unwrap();
            // PyObject_SetAttrString with a NULL value deletes the attribute
            // (this is what CPython's PyObject_DelAttrString macro expands to).
            assert_eq!(
                PyObject_SetAttrString(json_module, c_name.as_ptr(), std::ptr::null_mut()),
                0,
                "deleting json.loads should succeed"
            );
            let obj = bundle.json_loads(r#"{"key": "value"}"#);
            assert!(
                obj.is_null(),
                "json_loads should return NULL when json.loads is unavailable"
            );
        }
    }
    inner();
}

/// A non-string `PyObject` (e.g. an int) makes `PyUnicode_AsUTF8` fail and set
/// a `TypeError`. `to_string_py` must clear that stale error so it can't poison
/// later `PyErr_Occurred` checks on the same sub-interpreter (e.g. in
/// `bundle.run` / `json_loads`).
#[test]
fn test_to_string_py_clears_stale_error_for_non_string_object() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let int_obj = PyLong_FromUnsignedLongLong(42);
            assert!(!int_obj.is_null(), "int object should be created");
            let s = BundleInterface::to_string_py(int_obj);
            assert_eq!(s, "", "non-string object should convert to empty string");
            assert!(
                PyErr_Occurred().is_null(),
                "stale TypeError from PyUnicode_AsUTF8 must be cleared"
            );
            Py_DecRef(int_obj);
        }
    }
    inner();
}

/// The `json.dumps` attribute lookup can fail (e.g. when the `dumps`
/// attribute is removed from the json module), so `json_dumps` must return
/// Err (the "Failed to get json.dumps function" branch) instead of
/// dereferencing a null function pointer.
#[test]
fn test_json_dumps_returns_err_when_dumps_lookup_fails() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            // The json module is cached in the sub-interpreter, so this
            // returns the same module object `json_dumps` reads from.
            let json_module = PyImport_ImportModule(c"json".as_ptr());
            assert!(!json_module.is_null(), "json module should import");
            let c_name = CString::new("dumps").unwrap();
            // PyObject_SetAttrString with a NULL value deletes the attribute
            // (this is what CPython's PyObject_DelAttrString macro expands to).
            assert_eq!(
                PyObject_SetAttrString(json_module, c_name.as_ptr(), std::ptr::null_mut()),
                0,
                "deleting json.dumps should succeed"
            );
            let obj = bundle.json_loads(r#"{"key": "value"}"#);
            assert!(!obj.is_null(), "json_loads should succeed");
            let result = bundle.json_dumps(obj);
            BundleInterface::dispose_object(obj);
            assert_eq!(
                result,
                Err("Failed to get json.dumps function".to_string()),
                "json_dumps should return Err when json.dumps is unavailable"
            );
        }
    }
    inner();
}

/// A non-integer `PyObject` (e.g. a str) makes `PyLong_AsUnsignedLongLong`
/// fail and set a `TypeError`. `to_uint64` must clear that stale error so it
/// can't poison later `PyErr_Occurred` checks on the same sub-interpreter
/// (e.g. in `bundle.run` / `json_loads`).
#[test]
fn test_to_uint64_clears_stale_error_for_non_integer_object() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle
                .thread_scope()
                .expect("thread scope should be created");
            let str_obj = PyUnicode_FromString(c"not-an-int".as_ptr());
            assert!(!str_obj.is_null(), "str object should be created");
            let value = BundleInterface::to_uint64(str_obj);
            assert_eq!(value, 0, "non-integer object should convert to 0");
            assert!(
                PyErr_Occurred().is_null(),
                "stale TypeError from PyLong_AsUnsignedLongLong must be cleared"
            );
            Py_DecRef(str_obj);
        }
    }
    inner();
}

/// DIRECT UNIT TEST for `BundleManager::run_bundle_json`'s `json_dumps`
/// failure path.
///
/// A bundle function returning a non-JSON-serializable object (a set)
/// makes `json.dumps` raise `TypeError`. `BundleInterface::json_dumps`
/// returns `Err`, and `run_bundle_json` must return an `{"error": ...}`
/// object instead of panicking or returning `Value::Null`.
#[test]
fn test_run_bundle_json_returns_error_object_for_non_serializable_result() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {1, 2, 3}\n",
        );

        // No DB calls expected — the failure happens during serialization.
        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(
            result.get("error").is_some(),
            "expected an {{\"error\": ...}} object, got: {result}"
        );
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|s| s.contains("Failed to serialize result")),
            "expected serialization-failure message, got: {result}"
        );
    }
    inner();
}

/// DIRECT UNIT TEST for `BundleInterface::new` — reviewer request on
/// MR !220 ("Needs coverage." / "Please test the new branches.").
///
/// Success path: a valid bundle script on disk loads and `new` returns a
/// `BundleInterface` whose hash matches the requested bundle. This runs
/// through both new `PyDict_SetItemString` branches (setting `__builtins__`
/// and the `json` module in the globals dict).
#[test]
fn test_bundle_interface_new_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        let path_root = fixture.get_bundle_path().to_string_lossy().to_string();
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let bundle =
            unsafe { BundleInterface::new(&bundle_hash, &path_root) }.expect("bundle should load");
        assert_eq!(bundle.bundle_hash(), bundle_hash);
    }
    inner();
}

/// `BundleInterface::new` must return the "Failed to install stdout/stderr
/// redirection" error when `PyRun_StringFlags` returns NULL while running the
/// redirection script. The script always succeeds in practice, so the branch
/// is forced via the test-only `py_run_string_flags` override seam.
#[test]
fn test_bundle_interface_new_redirection_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        let path_root = fixture.get_bundle_path().to_string_lossy().to_string();
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );

        let prev = set_py_run_string_flags_override(Some(fail_redirect));
        let result = unsafe { BundleInterface::new(&bundle_hash, &path_root) };
        set_py_run_string_flags_override(prev);
        assert_eq!(
            result.err().as_deref(),
            Some("Failed to install stdout/stderr redirection"),
            "NULL PyRun_StringFlags result should fail with the redirection error"
        );
    }
    inner();
}

/// Override that makes `PyRun_StringFlags` always return NULL, forcing the
/// stdout/stderr redirection install failure branch in `BundleInterface::new`.
// SAFETY: Test-only; returns NULL without touching the arguments, so no
// reference handling is required.
unsafe fn fail_redirect(
    _code: *const c_char,
    _start: c_int,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
    _flags: *mut c_void,
) -> *mut PyObject {
    std::ptr::null_mut()
}

/// A NUL byte in the bundle path root makes `CString::new` fail, so
/// `BundleInterface::new` must return the "Bundle path contains NUL byte"
/// error instead of panicking. The constructor runs all the way through the
/// globals dict creation (including the new `PyDict_SetItemString`
/// `__builtins__` branch) before hitting this error.
#[test]
fn test_bundle_interface_new_nul_byte_in_path_root() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let bundle_hash = Uuid::new_v4().to_string();

        let result = unsafe { BundleInterface::new(&bundle_hash, "bad\0path") };
        assert_eq!(
            result.err().as_deref(),
            Some("Bundle path contains NUL byte"),
            "NUL byte in the bundle path root should be rejected"
        );
    }
    inner();
}

/// A bundle hash with no `bundle.py` on disk makes the bundle module import
/// fail, so `BundleInterface::new` must return the "Failed to load bundle
/// module" error. This runs through both new `PyDict_SetItemString` branches
/// (setting `__builtins__` and the `json` module) before the import fails.
#[test]
fn test_bundle_interface_new_missing_bundle_module() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        let path_root = fixture.get_bundle_path().to_string_lossy().to_string();

        let result = unsafe { BundleInterface::new(&bundle_hash, &path_root) };
        assert_eq!(
            result.err().as_deref(),
            Some("Failed to load bundle module"),
            "missing bundle module should fail with 'Failed to load bundle module'"
        );
    }
    inner();
}

/// DIRECT UNIT TEST — reviewer request on MR !200 ("Needs coverage." /
/// "Please test the new branches.").
///
/// Forces `traceback.format_tb` to fail by monkey-patching it to raise.
/// `PyObject_CallObject(tb_func, tb_args)` then returns NULL, so
/// `print_last_python_exception` takes the failure branch that logs the
/// "Error formatting python traceback frames" marker and swallows the
/// error before continuing to the exception-header formatting.
#[test]
fn test_print_last_python_exception_handles_format_tb_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let script = r#"
import traceback

def broken_format_tb(*args, **kwargs):
    raise RuntimeError("forced format_tb failure for regression test")

traceback.format_tb = broken_format_tb

def submit(details, job_data):
    raise RuntimeError("intentional failure for format_tb test")
    return {"ok": True}
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // The format_tb failure branch logs this marker.
        assert!(
            logs.contains("Error formatting python traceback frames"),
            "expected 'Error formatting python traceback frames' marker in logs, got:\n{logs}"
        );

        // The exception header must still be produced via the normal
        // format_exception_only path after the failure is swallowed.
        assert!(
            logs.contains("RuntimeError: intentional failure for format_tb test"),
            "expected final exception line in logs, got:\n{logs}"
        );
    }
    inner();
}

/// DIRECT UNIT TEST — reviewer request on MR !200.
///
/// Forces `traceback.format_exception_only` to fail by monkey-patching it
/// to raise. `PyObject_CallObject(eo_func, eo_args)` then returns NULL, so
/// `print_last_python_exception` takes the failure branch that synthesizes
/// a `type: value` header, ensuring the user is never left with no info.
#[test]
fn test_print_last_python_exception_handles_format_exception_only_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let script = r#"
import traceback

def broken_format_exception_only(*args, **kwargs):
    raise RuntimeError("forced format_exception_only failure for regression test")

traceback.format_exception_only = broken_format_exception_only

def submit(details, job_data):
    raise RuntimeError("intentional failure for format_exception_only test")
    return {"ok": True}
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        let logs = capture_logs(|| {
            let _ = BundleManager::singleton().run_bundle_json(
                "submit",
                &bundle_hash,
                &serde_json::json!({}),
                "",
            );
        });

        // The format_exception_only failure branch falls back to a
        // synthesized `type: value` header.
        assert!(
            logs.contains("RuntimeError: intentional failure for format_exception_only test"),
            "expected synthesized final exception line in logs, got:\n{logs}"
        );

        // The traceback frames must still be printed (format_tb ran before
        // the format_exception_only failure).
        assert!(
            logs.contains("Traceback (most recent call last):"),
            "expected traceback header in logs, got:\n{logs}"
        );
    }
    inner();
}

// ─── PyTuple_SetItem failure-branch tests (MR !315 reviewer requests) ────────
//
// The four `py_tuple_set_item` failure branches in
// `BundleInterface::print_last_python_exception` are unreachable through the
// public API: `PyTuple_SetItem` on a freshly-created tuple with a valid index
// always succeeds. These tests use the test-only FFI override seam
// (`set_py_tuple_set_item_override`) to force each branch and verify the
// defensive error handling.

/// RAII guard that installs a `py_tuple_set_item` override for the duration of
/// a test and restores the previous override on drop.
struct TupleSetItemOverrideGuard(Option<PyTupleSetItemFn>);

impl TupleSetItemOverrideGuard {
    fn install(f: PyTupleSetItemFn) -> Self {
        Self(set_py_tuple_set_item_override(Some(f)))
    }
}

impl Drop for TupleSetItemOverrideGuard {
    fn drop(&mut self) {
        set_py_tuple_set_item_override(self.0);
    }
}

/// Load a real bundle so the sub-interpreter has a live `traceback_module`,
/// which `print_last_python_exception` needs for its attribute lookups.
fn load_bundle_for_exception_printer() -> BundleInterface {
    crate::tests::init_python_global();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
    fixture.write_raw_script(
        &bundle_hash,
        "def submit(details, job_data):\n    return {}\n",
    );
    BundleManager::singleton()
        .load_bundle(&bundle_hash)
        .expect("bundle should load")
}

/// Fetch the `RuntimeError` exception class from the builtins.
// SAFETY: Caller holds PYTHON_MUTEX and the GIL; the returned object is a new
// reference owned by the caller.
unsafe fn runtime_error_type() -> *mut PyObject {
    let builtins = PyEval_GetBuiltins();
    let exc = PyDict_GetItemString(builtins, c"RuntimeError".as_ptr());
    assert!(
        !exc.is_null(),
        "RuntimeError should be available in builtins"
    );
    Py_IncRef(exc);
    exc
}

/// Override that fails `PyTuple_SetItem` only for size-1 tuples (the `tb_args`
/// tuple in `print_last_python_exception`). Mirrors `CPython`'s failure
/// behaviour: releases the item reference and returns -1.
// SAFETY: Test-only; `tuple`/`item` are live objects from the caller.
unsafe fn fail_size_one_tuple(tuple: *mut PyObject, pos: Py_ssize_t, item: *mut PyObject) -> c_int {
    if PyTuple_Size(tuple) == 1 {
        Py_DecRef(item);
        -1
    } else {
        PyTuple_SetItem(tuple, pos, item)
    }
}

/// Override that fails `PyTuple_SetItem` only for the size-2 `eo_args` tuple at
/// index 0 (the exception-type slot).
// SAFETY: Test-only; `tuple`/`item` are live objects from the caller.
unsafe fn fail_eo_args_index_zero(
    tuple: *mut PyObject,
    pos: Py_ssize_t,
    item: *mut PyObject,
) -> c_int {
    if PyTuple_Size(tuple) == 2 && pos == 0 {
        Py_DecRef(item);
        -1
    } else {
        PyTuple_SetItem(tuple, pos, item)
    }
}

/// Override that fails `PyTuple_SetItem` only for the size-2 `eo_args` tuple at
/// index 1 when the item is not `Py_None` (the non-NULL-value slot).
// SAFETY: Test-only; `tuple`/`item` are live objects from the caller.
unsafe fn fail_eo_args_non_none_item(
    tuple: *mut PyObject,
    pos: Py_ssize_t,
    item: *mut PyObject,
) -> c_int {
    if PyTuple_Size(tuple) == 2 && pos == 1 && item != my_py_none_struct() {
        Py_DecRef(item);
        -1
    } else {
        PyTuple_SetItem(tuple, pos, item)
    }
}

/// DIRECT UNIT TEST — reviewer request on MR !315.
///
/// Forces `py_tuple_set_item(tb_args, 0, traceback)` to fail. The branch logs
/// "Error setting traceback in args tuple" and continues to the exception
/// header, which must still be produced.
#[test]
fn test_print_last_python_exception_handles_tb_args_set_item_failure() {
    let bundle = load_bundle_for_exception_printer();
    let _override = TupleSetItemOverrideGuard::install(fail_size_one_tuple);

    let logs = capture_logs(|| {
        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            // Raise a Python exception WITH a traceback by running a snippet
            // that raises; the error indicator is left set.
            let globals = PyDict_New();
            assert!(!globals.is_null(), "PyDict_New should succeed");
            PyDict_SetItemString(globals, c"__builtins__".as_ptr(), PyEval_GetBuiltins());
            let code = c"def f():\n    raise RuntimeError('boom')\nf()";
            let _result = PyRun_StringFlags(
                code.as_ptr(),
                Py_file_input,
                globals,
                globals,
                std::ptr::null_mut(),
            );
            Py_DecRef(globals);
            bundle.print_last_python_exception();
        }
    });

    assert!(
        logs.contains("Error setting traceback in args tuple"),
        "expected 'Error setting traceback in args tuple' marker in logs, got:\n{logs}"
    );
    assert!(
        logs.contains("RuntimeError: boom"),
        "expected exception header after tb_args SetItem failure, got:\n{logs}"
    );
}

/// DIRECT UNIT TEST — reviewer request on MR !315.
///
/// Forces `py_tuple_set_item(eo_args, 0, extype)` to fail. The branch logs
/// "Error setting exception type in args tuple" and returns early.
#[test]
fn test_print_last_python_exception_handles_eo_args_type_set_item_failure() {
    let bundle = load_bundle_for_exception_printer();
    let _override = TupleSetItemOverrideGuard::install(fail_eo_args_index_zero);

    let logs = capture_logs(|| {
        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let exc = runtime_error_type();
            PyErr_SetString(exc, c"boom".as_ptr());
            Py_DecRef(exc);
            bundle.print_last_python_exception();
        }
    });

    assert!(
        logs.contains("Error setting exception type in args tuple"),
        "expected 'Error setting exception type in args tuple' marker in logs, got:\n{logs}"
    );
}

/// DIRECT UNIT TEST — reviewer request on MR !315.
///
/// Forces `py_tuple_set_item(eo_args, 1, value)` to fail when the exception has
/// a non-NULL value. The branch logs "Error setting exception value in args
/// tuple" and returns early.
#[test]
fn test_print_last_python_exception_handles_eo_args_value_set_item_failure() {
    let bundle = load_bundle_for_exception_printer();
    let _override = TupleSetItemOverrideGuard::install(fail_eo_args_non_none_item);

    let logs = capture_logs(|| {
        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let exc = runtime_error_type();
            PyErr_SetString(exc, c"boom".as_ptr());
            Py_DecRef(exc);
            bundle.print_last_python_exception();
        }
    });

    assert!(
        logs.contains("Error setting exception value in args tuple"),
        "expected 'Error setting exception value in args tuple' marker in logs, got:\n{logs}"
    );
}

/// DIRECT UNIT TEST for the `CString::new(job_data)` failure branch in
/// `BundleInterface::run` (reviewer comment on line 265).
///
/// Passing `job_data` containing an interior NUL byte makes `CString::new`
/// fail, so `run` must return `Err(NoneException)` before the bundle function
/// is called. `run_bundle_json` maps that to `Value::Null`.
#[test]
fn test_run_returns_null_when_job_data_contains_nul_byte() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        let script = r#"
def submit(details, job_data):
    return {"ok": True}
"#;
        fixture.write_raw_script(&bundle_hash, script);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);
        set_websocket_client(Arc::new(mock_ws));

        // An interior NUL byte makes CString::new fail, hitting the error
        // branch in BundleInterface::run before the bundle function runs.
        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "job_data_with\0interior_nul",
        );

        assert_eq!(result, serde_json::Value::Null);
    }
    inner();
}
