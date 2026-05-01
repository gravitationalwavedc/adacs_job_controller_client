#![allow(clippy::uninlined_format_args)]
use crate::bundle_logging::get_last_log_message;
use crate::bundle_manager::BundleManager;
use crate::tests::fixtures::bundle_fixture::BundleFixture;
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
