use crate::messaging::{Message, Priority, DB_RESPONSE};
use crate::bundle_manager::BundleManager;
use crate::websocket::{MockWebsocketClient, set_websocket_client};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use uuid::Uuid;
use serde_json::json;
use std::sync::Arc;
use mockall::predicate::*;

use crate::bundle_db::PyInit_bundledb;
use crate::bundle_logging::PyInit_bundlelogging;

static INIT: std::sync::Once = std::sync::Once::new();

async fn setup_test() {
    INIT.call_once(|| {
        crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
        unsafe {
            crate::python_interface::PyImport_AppendInittab(b"_bundledb\0".as_ptr() as *const std::os::raw::c_char, Some(PyInit_bundledb));
            crate::python_interface::PyImport_AppendInittab(b"_bundlelogging\0".as_ptr() as *const std::os::raw::c_char, Some(PyInit_bundlelogging));
        }
        crate::python_interface::init_python();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_create_or_update_job() {
    crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
    setup_test().await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    let script = r#"
import _bundledb
def submit(details, job_data):
    return _bundledb.create_or_update_job({"job_id": 4321, "submit_id": 1234, "working_directory": "/test/working/directory", "submit_directory": "/test/working/directory/submit"})
"#;
    fixture.write_raw_script(&bundle_hash, script);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|mut msg| {
            let _json_received = msg.pop_string();
            let db_request_id = msg.pop_ulong();
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_ulong(db_request_id);
            resp.push_bool(true); // protocol: success bool then json string
            resp.push_string(r#"{"job_id": 4321, "submit_id": 1234, "working_directory": "/test/working/directory", "submit_directory": "/test/working/directory/submit"}"#);
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
    crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
    setup_test().await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
def submit(details, job_data):
    try:
        return _bundledb.create_or_update_job({"test": 1})
    except Exception as e:
        return {"error": str(e)}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|mut msg| {
            let _ = msg.pop_string();
            let db_request_id = msg.pop_ulong();
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_ulong(db_request_id);
            resp.push_bool(false); // success = false
            resp.push_string("Internal Server Error");
            Box::pin(async move { Ok(resp) })
        });
    
    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert!(result["error"].as_str().unwrap().contains("Internal Server Error"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_job_by_id() {
    crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
    setup_test().await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
def submit(details, job_data):
    return _bundledb.get_job_by_id(1234)
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|mut msg| {
            let _ = msg.pop_ulong();
            let db_request_id = msg.pop_ulong();
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_ulong(db_request_id);
            resp.push_bool(true);
            resp.push_string(r#"{"id": 1234, "status": "running"}"#);
            Box::pin(async move { Ok(resp) })
        });
    
    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert_eq!(result["id"], 1234);
    assert_eq!(result["status"], "running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_delete_job_success() {
    crate::python_interface::load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
    setup_test().await;
    let fixture = BundleFixture::new();
    let bundle_hash = Uuid::new_v4().to_string();
    BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

    fixture.write_raw_script(&bundle_hash, r#"
import _bundledb
def submit(details, job_data):
    _bundledb.delete_job({"id": 1234})
    return {"status": "ok"}
"#);

    let mut mock_ws = MockWebsocketClient::new();
    mock_ws.expect_send_db_request()
        .times(1)
        .returning(|mut msg| {
            let _ = msg.pop_string();
            let db_request_id = msg.pop_ulong();
            let mut resp = Message::new(DB_RESPONSE, Priority::Highest, "database");
            resp.push_ulong(db_request_id);
            resp.push_bool(true); // success
            resp.push_string(""); // no data
            Box::pin(async move { Ok(resp) })
        });
    
    set_websocket_client(Arc::new(mock_ws));

    let result = unsafe {
        BundleManager::singleton().run_bundle_json("submit", &bundle_hash, &json!({}), "")
    };

    assert_eq!(result["status"], "ok");
}
