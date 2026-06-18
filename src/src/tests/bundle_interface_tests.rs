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

use crate::bundle_manager::BundleManager;
use crate::messaging::{Message, Priority, DB_RESPONSE};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use std::io::Write;
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
    }
    inner();
}
