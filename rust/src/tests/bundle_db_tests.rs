use crate::messaging::{Message, Priority, DB_RESPONSE};
use crate::bundle_manager::BundleManager;
use crate::websocket::{MockWebsocketClient, set_websocket_client};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use uuid::Uuid;
use serde_json::json;
use std::sync::Arc;

static INIT: std::sync::Once = std::sync::Once::new();

fn setup_test() {
    INIT.call_once(|| {
        crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
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

/// Create a mock response matching the C++ server response format.
/// parse_response in bundle_db will call from_data to skip the header.
fn make_db_response() -> Message {
    Message::new(DB_RESPONSE, Priority::Medium, "database")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_create_or_update_job() {
    setup_test();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    // Match the C++ Python script: create dict, call create_or_update_job, return dict
    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
import json

def submit(details, job_data):
    job = json.loads('{"submit_id": 1234, "working_directory": "/test/working/directory", "submit_directory": "/test/working/directory/submit"}')
    try:
        _bundledb.create_or_update_job(job)
        return job
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(true);     // success
            resp.push_ulong(4321);    // returned job_id
            Box::pin(async move { Ok(resp) })
        });

    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert_eq!(result["job_id"], 4321);
    assert_eq!(result["submit_id"], 1234);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_create_or_update_job_failure() {
    setup_test();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
import json

def submit(details, job_data):
    job = json.loads('{"test": 1}')
    try:
        _bundledb.create_or_update_job(job)
        return job
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(false);    // failure
            Box::pin(async move { Ok(resp) })
        });

    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert!(result["error"].as_str().unwrap().contains("unable to be created or updated"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_job_by_id() {
    setup_test();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb

def submit(details, job_data):
    try:
        return _bundledb.get_job_by_id(1234)
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(true);               // success
            resp.push_ulong(1234);              // job_id (echoed back, ignored by code)
            resp.push_string(r#"{"status": "running"}"#); // job data JSON
            Box::pin(async move { Ok(resp) })
        });

    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert_eq!(result["job_id"], 1234);
    assert_eq!(result["status"], "running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_job_by_id_failure() {
    setup_test();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb

def submit(details, job_data):
    try:
        return _bundledb.get_job_by_id(9999)
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(false);    // failure
            Box::pin(async move { Ok(resp) })
        });

    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert!(result["error"].as_str().unwrap().contains("does not exist"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_delete_job_success() {
    setup_test();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
import json

def submit(details, job_data):
    job = json.loads('{"job_id": 1234}')
    try:
        _bundledb.delete_job(job)
        return {"error": False}
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(true);     // success
            Box::pin(async move { Ok(resp) })
        });

    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert_eq!(result["error"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_delete_job_failure() {
    setup_test();
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
import json

def submit(details, job_data):
    job = json.loads('{"job_id": 1234}')
    try:
        _bundledb.delete_job(job)
        return {"error": False}
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(false);    // failure
            Box::pin(async move { Ok(resp) })
        });

    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert!(result["error"].as_str().unwrap().contains("does not exist"));
}
