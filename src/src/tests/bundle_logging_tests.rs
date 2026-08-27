#![allow(clippy::uninlined_format_args)]
use crate::bundle_logging::{get_last_log_message, write_log};
use crate::bundle_manager::BundleManager;
use crate::python_interface::{
    my_py_true_struct, PyErr_Occurred, PyLong_FromUnsignedLongLong, PyTuple_New, PyTuple_SetItem,
    PyUnicode_FromString, Py_DecRef, PYTHON_MUTEX,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::thread_bundle_map::clear_current_thread_bundle;
use serde_json::json;
use test_fork::test;
use uuid::Uuid;

fn setup() {
    crate::tests::init_python_global();
}

#[test]
fn test_simple_stdout() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    let test_message = "'testing stdout'";

    fixture.write_bundle_logging_std_out(&bundle_hash, test_message);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
    let last_log = get_last_log_message().expect("No log message captured");
    assert_eq!(
        last_log.0,
        format!("Bundle [{}]: testing stdout", bundle_hash)
    );
    assert!(last_log.1); // is_stdout
}

#[test]
fn test_complex_stdout() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    let test_message =
        "'testing stdout', 56, {'a': 'b'}, [45, 'a', sum([5, 4])], (123, 321,), type((1,))";

    fixture.write_bundle_logging_std_out(&bundle_hash, test_message);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
    let last_log = get_last_log_message().expect("No log message captured");
    assert_eq!(
        last_log.0,
        format!(
            "Bundle [{}]: testing stdout 56 {{'a': 'b'}} [45, 'a', 9] (123, 321) <class 'tuple'>",
            bundle_hash
        )
    );
    assert!(last_log.1); // is_stdout
}

#[test]
fn test_simple_stderr() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    let test_message = "'testing stderr'";

    fixture.write_bundle_logging_std_err(&bundle_hash, test_message);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
    let last_log = get_last_log_message().expect("No log message captured");
    assert_eq!(
        last_log.0,
        format!("Bundle [{}]: testing stderr", bundle_hash)
    );
    assert!(!last_log.1); // is_stdout is false
}

#[test]
fn test_complex_stderr() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    let test_message =
        "'testing stderr', 56, {'a': 'b'}, [45, 'a', sum([5, 4])], (123, 321,), type((1,))";

    fixture.write_bundle_logging_std_err(&bundle_hash, test_message);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
    let last_log = get_last_log_message().expect("No log message captured");
    assert_eq!(
        last_log.0,
        format!(
            "Bundle [{}]: testing stderr 56 {{'a': 'b'}} [45, 'a', 9] (123, 321) <class 'tuple'>",
            bundle_hash
        )
    );
    assert!(!last_log.1); // is_stdout is false
}

#[test]
fn test_stdout_during_load() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    let test_message = "'testing stdout load'";

    fixture.write_bundle_logging_std_out_during_load(&bundle_hash, test_message);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
    let last_log = get_last_log_message().expect("No log message captured");
    assert_eq!(
        last_log.0,
        format!("Bundle [{}]: testing stdout load", bundle_hash)
    );
    assert!(last_log.1); // is_stdout
}

#[test]
fn test_stderr_during_load() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    let test_message = "'testing stdout load'";

    fixture.write_bundle_logging_std_err_during_load(&bundle_hash, test_message);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
    let last_log = get_last_log_message().expect("No log message captured");
    assert_eq!(
        last_log.0,
        format!("Bundle [{}]: testing stdout load", bundle_hash)
    );
    assert!(!last_log.1); // is_stdout is false
}

#[test]
fn test_wrong_arity_write_returns_none_cleanly() {
    setup();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();

    fixture.write_bundle_logging_wrong_arity(&bundle_hash);

    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
    let result =
        BundleManager::singleton().run_bundle_bool("logging_test", &bundle_hash, &json!({}), "");

    assert!(result);
}

/// DIRECT UNIT TEST for the `c_msg.is_null()` branch in `write_log` —
/// reviewer request on MR !289 ("Please add a unit test to cover this branch.").
/// A non-string `PyObject` (e.g. an int) makes `PyUnicode_AsUTF8` fail and set a
/// `TypeError`. `write_log` must clear that stale error (`PyErr_Clear`) and return
/// `Py_None` instead of returning with an exception set.
#[test]
fn test_write_log_clears_stale_error_for_non_string_message() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let args = PyTuple_New(2);
            assert!(!args.is_null(), "PyTuple_New(2) should succeed");
            // is_stdout = True; message = an int, which makes PyUnicode_AsUTF8
            // fail and set a TypeError.
            assert_eq!(
                PyTuple_SetItem(args, 0, my_py_true_struct()),
                0,
                "setting is_stdout should succeed"
            );
            let int_obj = PyLong_FromUnsignedLongLong(42);
            assert!(!int_obj.is_null(), "int object should be created");
            assert_eq!(
                PyTuple_SetItem(args, 1, int_obj),
                0,
                "setting message should succeed"
            );
            let result = write_log(std::ptr::null_mut(), args);
            assert!(
                !result.is_null(),
                "write_log should return Py_None for a non-string message"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "stale TypeError from PyUnicode_AsUTF8 must be cleared"
            );
            Py_DecRef(result);
            Py_DecRef(args);
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `get_current_thread_bundle().unwrap_or_else(||
/// "unknown".to_string())` fallback in `write_log` (`bundle_logging.rs:41`).
/// The existing tests all run inside a bundle with an active
/// `ThreadBundleGuard`, so this "unknown" branch is never exercised. This test
/// clears the current thread bundle, calls `write_log` directly, and asserts
/// the captured message uses the "unknown" bundle hash.
#[test]
fn test_write_log_uses_unknown_bundle_hash_when_no_thread_bundle() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().into_owned());
        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            // No ThreadBundleGuard is created, so get_current_thread_bundle()
            // returns None and write_log must fall back to "unknown".
            clear_current_thread_bundle();

            let write_log_msg = |msg: &str| {
                // SAFETY: PYTHON_MUTEX is held and the ThreadScope holds the GIL
                // for the bundle's sub-interpreter, so the Python C-API calls
                // below are valid (the closure inherits the outer unsafe block).
                let c_msg = std::ffi::CString::new(msg).expect("message has no NUL");
                let args = PyTuple_New(2);
                assert!(!args.is_null(), "PyTuple_New(2) should succeed");
                assert_eq!(
                    PyTuple_SetItem(args, 0, my_py_true_struct()),
                    0,
                    "setting is_stdout should succeed"
                );
                let msg_obj = PyUnicode_FromString(c_msg.as_ptr());
                assert!(!msg_obj.is_null(), "message string should be created");
                assert_eq!(
                    PyTuple_SetItem(args, 1, msg_obj),
                    0,
                    "setting message should succeed"
                );
                let result = write_log(std::ptr::null_mut(), args);
                assert!(!result.is_null(), "write_log should return Py_None");
                Py_DecRef(result);
                Py_DecRef(args);
            };

            // Flush any line parts left over from a previous test so this test
            // is deterministic regardless of test execution order.
            write_log_msg("\n");
            write_log_msg("hello");
            write_log_msg("\n");
        }

        let last_log = get_last_log_message().expect("No log message captured");
        assert_eq!(last_log.0, "Bundle [unknown]: hello");
        assert!(last_log.1); // is_stdout
    }
    inner();
}
