use crate::bundle_manager::BundleManager;
use crate::messaging::{Message, Priority, DB_RESPONSE};
use crate::tests::fixtures::bundle_fixture::BundleFixture;
use crate::websocket::{set_websocket_client, MockWebsocketClient};
use std::sync::Arc;
use test_fork::test;
use uuid::Uuid;

/// Create a mock response matching the C++ server response format.
/// `parse_response` in `bundle_db` will call `from_data` to skip the header.
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
            resp.push_bool(true); // success
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
            resp.push_bool(false); // failure
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
            resp.push_bool(true); // success
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
            resp.push_bool(false); // failure
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
fn test_delete_job_success() {
    #[tokio::main(flavor = "current_thread")]
    async fn inner() {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());

        fixture.write_bundle_db_delete_job(&bundle_hash, r#"{"job_id": 1234}"#);

        let mut mock_ws = MockWebsocketClient::new();
        mock_ws.expect_send_db_request().times(1).returning(|_msg| {
            let mut resp = make_db_response();
            resp.push_bool(true); // success
            Box::pin(async move { Ok(resp) })
        });

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
            let mut resp = make_db_response();
            resp.push_bool(false); // failure
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
