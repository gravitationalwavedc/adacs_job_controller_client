use crate::bundle_db::{create_or_update_job, delete_job, get_job_by_id, set_bundle_db_error};
use crate::bundle_interface::BundleInterface;
use crate::bundle_manager::BundleManager;
use crate::messaging::{Message, Priority, DB_RESPONSE};
use crate::python_interface::{
    PyDict_New, PyDict_SetItemString, PyErr_Occurred, PyLong_FromUnsignedLongLong, PyTuple_New,
    PyTuple_SetItem, Py_DecRef, PYTHON_MUTEX,
};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::thread_bundle_map::ThreadBundleGuard;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use std::ptr;
use std::sync::Arc;
use test_fork::test;
use uuid::Uuid;

/// Create a mock response matching the C++ server response format.
/// `parse_response` in `bundle_db` reads from the current cursor position.
fn make_db_response() -> Message {
    Message::new(DB_RESPONSE, Priority::Medium, "database")
}

#[test]
fn test_create_or_update_job() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_create_or_update_job(
            &bundle_hash,
            r#"{"submit_id": 1234, "working_directory": "/test/working/directory", "submit_directory": "/test/working/directory/submit"}"#,
        );

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_ulong(4321); // returned job_id
            Box::pin(async move { Ok(resp) })
        });

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert_eq!(result["job_id"], 4321);
        assert_eq!(result["submit_id"], 1234);
    }
    inner();
}

#[test]
fn test_create_or_update_job_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_create_or_update_job(&bundle_hash, r#"{"test": 1}"#);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_ulong(0); // failure
            Box::pin(async move { Ok(resp) })
        });

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("unable to be created or updated"));
    }
    inner();
}

#[test]
fn test_get_job_by_id() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_get_job_by_id(&bundle_hash, 1234);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_uint(1); // count
            resp.push_ulong(1234); // job_id (echoed back, ignored by code)
            resp.push_string(r#"{"status": "running"}"#); // job data JSON
            Box::pin(async move { Ok(resp) })
        });

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert_eq!(result["job_id"], 1234);
        assert_eq!(result["status"], "running");
    }
    inner();
}

#[test]
fn test_get_job_by_id_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_get_job_by_id(&bundle_hash, 9999);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_uint(0); // failure
            Box::pin(async move { Ok(resp) })
        });

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(result["error"].as_str().unwrap().contains("does not exist"));
    }
    inner();
}

#[test]
fn test_get_job_by_id_malformed_json() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_get_job_by_id(&bundle_hash, 1234);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_uint(1); // count
            resp.push_ulong(1234); // job_id (echoed back, ignored by code)
            resp.push_string(r"{invalid json"); // malformed job data JSON
            Box::pin(async move { Ok(resp) })
        });

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Failed to parse job data JSON"));
    }
    inner();
}

#[test]
fn test_get_job_by_id_non_integer_job_id() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_get_job_by_id_str(&bundle_hash, "not_an_integer");

        let mut mock_ws = MockWebsocketClient::new();
        // No DB request expected - validation happens before Websocket call
        mock_ws.expect_send_db_request().times(0);

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Job ID must be an integer"));
    }
    inner();
}

#[test]
fn test_delete_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_delete_job(&bundle_hash, r#"{"job_id": 1234}"#);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws
            .expect_send_db_request()
            .times(1)
            .returning(|_msg| Box::pin(async move { Ok(make_db_response()) }));

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert_eq!(result["error"], false);
    }
    inner();
}

#[test]
fn test_delete_job_failure() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_delete_job(&bundle_hash, r#"{"job_id": 1234}"#);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            Box::pin(async move {
                Err::<Message, Box<dyn std::error::Error + Send + Sync>>("delete failed".into())
            })
        });

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(result["error"].as_str().unwrap().contains("delete failed"));
    }
    inner();
}

#[test]
fn test_create_or_update_job_json_dumps_failure_raises_bundledb_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // A self-referential dict makes json.dumps raise ValueError. The FFI
        // callback must surface this as `_bundledb.error`, not return NULL
        // without an exception set (which CPython reports as SystemError).
        fixture.write_raw_script(
            &bundle_hash,
            r#"
import _bundledb

def submit(details, job_data):
    job = {}
    job["self"] = job
    try:
        _bundledb.create_or_update_job(job)
        return {"error": "no exception raised"}
    except Exception as e:
        return {"error": type(e).__module__ + "." + type(e).__name__}
"#,
        );

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert_eq!(result["error"], "_bundledb.error");
    }
    inner();
}

#[test]
fn test_delete_job_json_dumps_failure_raises_bundledb_error() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_raw_script(
            &bundle_hash,
            r#"
import _bundledb

def submit(details, job_data):
    job = {}
    job["self"] = job
    try:
        _bundledb.delete_job(job)
        return {"error": "no exception raised"}
    except Exception as e:
        return {"error": type(e).__module__ + "." + type(e).__name__}
"#,
        );

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(0);

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert_eq!(result["error"], "_bundledb.error");
    }
    inner();
}

#[test]
fn test_delete_job_failure_job_id_must_be_provided() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        // Test case 1: job_id field absent
        fixture.write_bundle_db_delete_job(&bundle_hash, r#"{"other_field": 1234}"#);

        let mut mock_ws = MockWebsocketClient::new();
        // No DB request expected - validation happens before Websocket call
        mock_ws.expect_send_db_request().times(0);

        set_websocket_client(Arc::new(mock_ws));

        let result = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash,
            &serde_json::json!({}),
            "",
        );

        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Job ID must be provided."));

        // Test case 2: job_id = 0
        let bundle_hash_2 = Uuid::new_v4().to_string();
        fixture.write_bundle_db_delete_job(&bundle_hash_2, r#"{"job_id": 0}"#);

        let mut mock_ws_2 = MockWebsocketClient::new();
        mock_ws_2.expect_send_db_request().times(0);

        set_websocket_client(Arc::new(mock_ws_2));

        let result_2 = BundleManager::singleton().run_bundle_json(
            "submit",
            &bundle_hash_2,
            &serde_json::json!({}),
            "",
        );

        assert!(result_2["error"]
            .as_str()
            .unwrap()
            .contains("Job ID must be provided."));
    }
    inner();
}

/// DIRECT UNIT TEST for the `PyTuple_GetItem` null-guard branches in the
/// `_bundledb` FFI callbacks — reviewer request on MR !202 ("Please test the
/// new branches."). Calling `create_or_update_job` with an empty args tuple
/// makes `PyTuple_GetItem(args, 0)` return NULL, exercising the early-return
/// guard that prevents a wrong-arity call from segfaulting the daemon.
#[test]
fn test_create_or_update_job_rejects_empty_args() {
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

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let empty_args = PyTuple_New(0);
            assert!(!empty_args.is_null(), "PyTuple_New(0) should succeed");
            let result = create_or_update_job(std::ptr::null_mut(), empty_args);
            Py_DecRef(empty_args);
            assert!(
                result.is_null(),
                "empty args tuple should hit the invalid-arguments guard"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `PyTuple_GetItem` null-guard branch in
/// `get_job_by_id` — reviewer request on MR !202 ("Please test the new
/// branches."). An empty args tuple makes `PyTuple_GetItem(args, 0)` return
/// NULL, so the callback must return null instead of dereferencing the result.
#[test]
fn test_get_job_by_id_rejects_empty_args() {
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

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let empty_args = PyTuple_New(0);
            assert!(!empty_args.is_null(), "PyTuple_New(0) should succeed");
            let result = get_job_by_id(std::ptr::null_mut(), empty_args);
            Py_DecRef(empty_args);
            assert!(
                result.is_null(),
                "empty args tuple should hit the invalid-arguments guard"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `PyTuple_GetItem` null-guard branch in
/// `delete_job` — reviewer request on MR !202 ("Please test the new branches.").
/// An empty args tuple makes `PyTuple_GetItem(args, 0)` return NULL, so the
/// callback must return null instead of dereferencing the result.
#[test]
fn test_delete_job_rejects_empty_args() {
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

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let empty_args = PyTuple_New(0);
            assert!(!empty_args.is_null(), "PyTuple_New(0) should succeed");
            let result = delete_job(std::ptr::null_mut(), empty_args);
            Py_DecRef(empty_args);
            assert!(
                result.is_null(),
                "empty args tuple should hit the invalid-arguments guard"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `get_bundle_db_error` NULL-guard branch in
/// `create_or_update_job` — reviewer request on MR !276 ("Please add a unit
/// test to cover this branch."). Storing a NULL error object for the bundle
/// hash makes `get_bundle_db_error` return NULL, so the callback must return
/// NULL without setting a Python error (the guard fires before any
/// `PyErr_SetString` call).
#[test]
fn test_create_or_update_job_returns_null_when_error_object_missing() {
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
        BundleManager::initialize(path_root.clone());
        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let _bundle_guard = ThreadBundleGuard::new(bundle_hash.clone());
            set_bundle_db_error(&bundle_hash, ptr::null_mut());
            let dict = PyDict_New();
            let args = PyTuple_New(1);
            assert_eq!(
                PyTuple_SetItem(args, 0, dict),
                0,
                "tuple set should succeed"
            );
            let result = create_or_update_job(ptr::null_mut(), args);
            Py_DecRef(args);
            assert!(
                result.is_null(),
                "NULL error object should hit the get_bundle_db_error guard"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "guard must return NULL without setting a Python error"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `get_bundle_db_error` NULL-guard branch in
/// `get_job_by_id` — reviewer request on MR !276 ("Please add a unit test to
/// cover this branch."). Storing a NULL error object for the bundle hash makes
/// `get_bundle_db_error` return NULL, so the callback must return NULL without
/// setting a Python error (the guard fires before any `PyErr_SetString` call).
#[test]
fn test_get_job_by_id_returns_null_when_error_object_missing() {
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
        BundleManager::initialize(path_root.clone());
        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let _bundle_guard = ThreadBundleGuard::new(bundle_hash.clone());
            set_bundle_db_error(&bundle_hash, ptr::null_mut());
            let job_id_obj = PyLong_FromUnsignedLongLong(42);
            let args = PyTuple_New(1);
            assert_eq!(
                PyTuple_SetItem(args, 0, job_id_obj),
                0,
                "tuple set should succeed"
            );
            let result = get_job_by_id(ptr::null_mut(), args);
            Py_DecRef(args);
            assert!(
                result.is_null(),
                "NULL error object should hit the get_bundle_db_error guard"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "guard must return NULL without setting a Python error"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `get_bundle_db_error` NULL-guard branch in
/// `delete_job`'s missing-job-id path — reviewer request on MR !276 ("Please
/// add a unit test to cover this branch."). With no `job_id` in the dict the
/// callback reaches the `job_id == 0` branch; a NULL error object there must
/// make it return NULL without setting a Python error.
#[test]
fn test_delete_job_returns_null_when_error_object_missing_for_missing_job_id() {
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
        BundleManager::initialize(path_root.clone());
        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let _bundle_guard = ThreadBundleGuard::new(bundle_hash.clone());
            set_bundle_db_error(&bundle_hash, ptr::null_mut());
            let dict = PyDict_New();
            let args = PyTuple_New(1);
            assert_eq!(
                PyTuple_SetItem(args, 0, dict),
                0,
                "tuple set should succeed"
            );
            let result = delete_job(ptr::null_mut(), args);
            Py_DecRef(args);
            assert!(
                result.is_null(),
                "NULL error object should hit the get_bundle_db_error guard"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "guard must return NULL without setting a Python error"
            );
        }
    }
    inner();
}

/// DIRECT UNIT TEST for the `get_bundle_db_error` NULL-guard branch in
/// `delete_job`'s send-failure path — reviewer request on MR !276 ("Please add
/// a unit test to cover this branch."). When the DB request fails and the error
/// object lookup returns NULL, the callback must return NULL without setting a
/// Python error.
#[test]
fn test_delete_job_returns_null_when_error_object_missing_on_send_failure() {
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
        BundleManager::initialize(path_root.clone());
        let bundle = BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load");

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            Box::pin(async move {
                Err::<Message, Box<dyn std::error::Error + Send + Sync>>("delete failed".into())
            })
        });
        set_websocket_client(Arc::new(mock_ws));

        let _guard = PYTHON_MUTEX.lock();
        // SAFETY: PYTHON_MUTEX is held and the ThreadScope acquires the GIL for
        // the bundle's sub-interpreter, so the Python C-API calls below are valid.
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let _bundle_guard = ThreadBundleGuard::new(bundle_hash.clone());
            set_bundle_db_error(&bundle_hash, ptr::null_mut());
            let dict = PyDict_New();
            let job_id_value = PyLong_FromUnsignedLongLong(1234);
            assert_eq!(
                PyDict_SetItemString(dict, c"job_id".as_ptr(), job_id_value),
                0,
                "dict set should succeed"
            );
            Py_DecRef(job_id_value);
            let args = PyTuple_New(1);
            assert_eq!(
                PyTuple_SetItem(args, 0, dict),
                0,
                "tuple set should succeed"
            );
            let result = delete_job(ptr::null_mut(), args);
            Py_DecRef(args);
            assert!(
                result.is_null(),
                "NULL error object should hit the get_bundle_db_error guard"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "guard must return NULL without setting a Python error"
            );
        }
    }
    inner();
}
