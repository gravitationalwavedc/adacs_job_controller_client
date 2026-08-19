//! Port of C++ `BundleDB`.
//!
//! Provides the `_bundledb` Python module with `create_or_update_job`, `get_job_by_id`,
//! and `delete_job` methods that communicate with the server via WebSocket.
//!
//! Each method:
//!  1. Gets the bundle hash from the thread bundle map
//!  2. Loads the bundle from `BundleManager` (always a cache hit)
//!  3. Builds a Message matching the C++ wire protocol
//!  4. Sends via the async WebSocket client using a sync→async bridge
//!  5. Processes the response and returns appropriate Python objects

use crate::bundle_manager::BundleManager;
use crate::messaging::{
    Message, Priority, DB_BUNDLE_CREATE_OR_UPDATE_JOB, DB_BUNDLE_DELETE_JOB,
    DB_BUNDLE_GET_JOB_BY_ID,
};
use crate::python_interface::{
    return_py_none, PyDict_SetItemString, PyErr_Clear, PyErr_NewException, PyErr_Occurred,
    PyErr_SetString, PyLong_AsUnsignedLongLong, PyLong_FromUnsignedLongLong, PyMethodDef,
    PyModuleDef, PyModuleDef_Base, PyModule_AddObject, PyModule_Create2, PyObject, PyObject_Head,
    PyTuple_GetItem, Py_DecRef, Py_IncRef, Py_XDECREF, METH_VARARGS, PYTHON_API_VERSION,
};
use crate::thread_bundle_map::get_current_thread_bundle;
use crate::websocket::get_websocket_client;
use std::collections::HashMap;
use std::ffi::CString;
use std::ptr;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, error, trace, warn};

/// Wrapper around `*mut PyObject` that implements `Send` (needed for `Mutex` storage).
/// Safety: all access to the stored pointer is serialized through the mutex and `PYTHON_MUTEX`.
struct SendPyObject(*mut crate::python_interface::PyObject);
unsafe impl Send for SendPyObject {}

/// Per-bundle-hash error exceptions. Each sub-interpreter gets its own
/// `_bundledb.error` exception object during module init, stored here
/// so callbacks can reference the correct one for their bundle.
static BUNDLE_DB_ERRORS: OnceLock<Mutex<HashMap<String, SendPyObject>>> = OnceLock::new();

fn get_bundle_db_error(bundle_hash: &str) -> *mut crate::python_interface::PyObject {
    let mut errors = BUNDLE_DB_ERRORS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(exc) = errors.get(bundle_hash) {
        return exc.0;
    }
    // Fallback: create a generic RuntimeError if no exception was stored
    // for this bundle hash. This should not happen in normal operation.
    // SAFETY: PyErr_NewException is called with a valid C string pointer
    // and null parent/base dicts, which is always safe.
    let err = unsafe {
        crate::python_interface::PyErr_NewException(
            c"_bundledb.error".as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if err.is_null() {
        return ptr::null_mut();
    }
    // Cache the fallback so each bundle hash gets exactly one exception
    // object instead of leaking a new one on every call.
    errors.insert(bundle_hash.to_string(), SendPyObject(err));
    err
}

fn set_bundle_db_error(bundle_hash: &str, exc: *mut crate::python_interface::PyObject) {
    let mut errors = BUNDLE_DB_ERRORS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    errors.insert(bundle_hash.to_string(), SendPyObject(exc));
}

fn send_and_wait(msg: Message) -> Result<Message, String> {
    let msg_id = msg.id;
    debug!("bundle_db: send_and_wait - msg_id={}", msg_id);
    // Use the persistent DbBridge if available (production path),
    // otherwise fall back to thread-per-call (test paths without DbBridge).
    if let Some(bridge) = crate::db_bridge::DbBridge::try_get() {
        trace!("bundle_db: using DbBridge for request");
        return bridge.send(msg);
    }
    trace!("bundle_db: using fallback thread-per-call for request");
    let ws = get_websocket_client();
    let fut = ws.send_db_request(msg);
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create tokio runtime: {e}"))?
            .block_on(fut)
            .map_err(|e| format!("DB request error: {e}"))
    })
    .join()
    .map_err(|_| "DB request thread panicked".to_string())?
}

/// Re-parse a response Message so the read index is past the header.
fn parse_response(response: &Message) -> Message {
    response.clone_for_payload_reading()
}

fn build_bundle_create_or_update_message(
    bundle_hash: &str,
    job_id: u64,
    job_data_json: &str,
) -> Message {
    let mut msg = Message::new(DB_BUNDLE_CREATE_OR_UPDATE_JOB, Priority::Medium, "database");
    msg.push_ulong(job_id);
    msg.push_string(job_data_json);
    msg.push_string(bundle_hash);
    msg
}

fn build_bundle_get_by_id_message(job_id: u64) -> Message {
    let mut msg = Message::new(DB_BUNDLE_GET_JOB_BY_ID, Priority::Medium, "database");
    msg.push_ulong(job_id);
    msg
}

fn build_bundle_delete_message(job_id: u64) -> Message {
    let mut msg = Message::new(DB_BUNDLE_DELETE_JOB, Priority::Medium, "database");
    msg.push_ulong(job_id);
    msg
}

fn parse_create_or_update_response(response: &Message) -> Result<u64, String> {
    let mut resp = parse_response(response);
    let new_job_id = resp.pop_ulong();
    if new_job_id == 0 {
        return Err("Job was unable to be created or updated.".to_string());
    }
    Ok(new_job_id)
}

fn parse_get_job_by_id_response(
    response: &Message,
    requested_job_id: u64,
) -> Result<String, String> {
    let mut resp = parse_response(response);
    let count = resp.pop_uint();
    if count == 0 {
        return Err(format!("Job with ID {requested_job_id} does not exist."));
    }

    let _resp_job_id = resp.pop_ulong();
    Ok(resp.pop_string())
}

/// Build a NUL-terminated C string for an FFI error message.
///
/// Server-provided error text may contain an interior NUL byte, which would
/// make `CString::new` panic (AGENTS.md rule #2: never panic on untrusted
/// input). Fall back to a static message in that case.
fn err_cstring(msg: &str) -> CString {
    CString::new(msg).unwrap_or_else(|_| {
        CString::new("Internal error").expect("static fallback has no NUL byte")
    })
}

/// Load the current thread's bundle and extract the job's `job_id` from the
/// job data dict.
///
/// Shared prologue of the `create_or_update_job` and `delete_job` FFI
/// callbacks: resolve the bundle hash, load the bundle (logging and returning
/// `None` on failure), dump the job dict to JSON, and parse the `job_id`
/// field (defaulting to `0` when absent).
///
/// Returns `(bundle_hash, job_id, job_data)` on success.
///
/// # Safety
/// `dict` must be a valid `PyObject` pointer obtained from the `args` tuple.
unsafe fn load_bundle_and_job_id(dict: *mut PyObject) -> Option<(String, u64, serde_json::Value)> {
    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    let bundle = match BundleManager::singleton().load_bundle(&bundle_hash) {
        Ok(b) => b,
        Err(e) => {
            error!(
                "DB: Bundle {} not found in cache during FFI callback: {}",
                bundle_hash, e
            );
            return None;
        }
    };

    let Ok(json_str) = bundle.json_dumps(dict) else {
        error!(
            "DB: failed to serialize job data for bundle hash: {}",
            bundle_hash
        );
        let error_obj = get_bundle_db_error(&bundle_hash);
        let err_msg = err_cstring("Failed to serialize job data");
        PyErr_SetString(error_obj, err_msg.as_ptr());
        return None;
    };
    let job_data: serde_json::Value =
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null);

    let job_id = job_data
        .get("job_id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    Some((bundle_hash, job_id, job_data))
}

/// Set a Python error on `error_obj` and return the null pointer that FFI
/// callbacks must return on failure.
fn set_db_error_and_return_null(
    error_obj: *mut crate::python_interface::PyObject,
    msg: &str,
) -> *mut PyObject {
    let err_msg = err_cstring(msg);
    // SAFETY: `error_obj` is a valid exception object obtained from
    // `get_bundle_db_error`, and `err_msg` is a NUL-terminated C string.
    unsafe { PyErr_SetString(error_obj, err_msg.as_ptr()) };
    ptr::null_mut()
}

/// Set `job_id` in a Python dict, handling a failed `PyLong_FromUnsignedLongLong`
/// allocation and a failed `PyDict_SetItemString`. Returns `false` (and sets the
/// Python error) when `value` is NULL or the dict insertion fails.
///
/// `context` names the caller for the error log (e.g. `"create_or_update_job"`).
///
/// # Safety
/// `dict` must be a valid dict object; `value` is either NULL or a valid
/// `PyObject` reference owned by the caller; `error_obj` must be a valid
/// exception object.
unsafe fn set_job_id_in_dict(
    dict: *mut PyObject,
    value: *mut PyObject,
    context: &str,
    bundle_hash: &str,
    job_id: u64,
    error_obj: *mut PyObject,
) -> bool {
    if value.is_null() {
        error!(
            "DB: {} failed to allocate job_id for bundle hash: {}, jobId: {}",
            context, bundle_hash, job_id
        );
        let err_msg = err_cstring("Failed to allocate job_id");
        PyErr_SetString(error_obj, err_msg.as_ptr());
        return false;
    }
    if PyDict_SetItemString(dict, c"job_id".as_ptr(), value) < 0 {
        Py_DecRef(value);
        error!(
            "DB: {} failed to set job_id in dict for bundle hash: {}, jobId: {}",
            context, bundle_hash, job_id
        );
        let err_msg = err_cstring("Failed to set job_id in dict");
        PyErr_SetString(error_obj, err_msg.as_ptr());
        return false;
    }
    Py_DecRef(value);
    true
}

// SAFETY: Called by Python C API with a valid `args` tuple pointer.
// All FFI calls (PyTuple_GetItem, PyDict_SetItemString, PyLong_*, etc.)
// operate on pointers derived from `args` or freshly created Python objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn create_or_update_job(
    _self: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    let dict = PyTuple_GetItem(args, 0);
    if dict.is_null() {
        error!("DB: create_or_update_job - invalid arguments");
        return ptr::null_mut();
    }

    let Some((bundle_hash, job_id, job_data)) = load_bundle_and_job_id(dict) else {
        return ptr::null_mut();
    };

    // Remove job_id from the data
    let mut job_data_clean = job_data.clone();
    if let serde_json::Value::Object(ref mut map) = job_data_clean {
        map.remove("job_id");
    }

    let msg = build_bundle_create_or_update_message(
        &bundle_hash,
        job_id,
        &serde_json::to_string(&job_data_clean).unwrap_or_default(),
    );

    debug!(
        "DB: create_or_update_job req - bundle hash: {}, jobId: {}",
        bundle_hash, job_id
    );
    trace!(
        "DB: create_or_update_job - job_data_clean: {}",
        serde_json::to_string(&job_data_clean).unwrap_or_default()
    );

    let error_obj = get_bundle_db_error(&bundle_hash);
    match send_and_wait(msg) {
        Ok(response) => {
            let new_job_id = match parse_create_or_update_response(&response) {
                Ok(id) => id,
                Err(message) => {
                    error!(
                        "DB: create_or_update_job parse error for bundle hash: {}, jobId: {}: {}",
                        bundle_hash, job_id, message
                    );
                    return set_db_error_and_return_null(error_obj, &message);
                }
            };

            // Set job_id in the original dict (matches C++ exactly)
            let value = PyLong_FromUnsignedLongLong(new_job_id);
            if !set_job_id_in_dict(
                dict,
                value,
                "create_or_update_job",
                &bundle_hash,
                new_job_id,
                error_obj,
            ) {
                return ptr::null_mut();
            }

            debug!("DB: create_or_update_job res - jobId: {}", new_job_id);

            return_py_none()
        }
        Err(e) => {
            error!(
                "DB: create_or_update_job error for bundle hash: {}, jobId: {}: {}",
                bundle_hash, job_id, e
            );
            set_db_error_and_return_null(error_obj, &format!("DB error: {e}"))
        }
    }
}

// SAFETY: Called by Python C API with a valid `args` tuple pointer.
// All FFI calls (PyTuple_GetItem, PyLong_AsUnsignedLongLong, PyDict_SetItemString,
// etc.) operate on pointers derived from `args` or freshly created Python objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_job_by_id(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let job_id_obj = PyTuple_GetItem(args, 0);
    if job_id_obj.is_null() {
        error!("DB: get_job_by_id - invalid arguments");
        return ptr::null_mut();
    }
    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());

    let job_id = PyLong_AsUnsignedLongLong(job_id_obj);
    if !PyErr_Occurred().is_null() {
        error!(
            "DB: get_job_by_id error for bundle hash: {} - job ID must be an integer",
            bundle_hash
        );
        PyErr_Clear();
        let error_obj = get_bundle_db_error(&bundle_hash);
        PyErr_SetString(error_obj, c"Job ID must be an integer".as_ptr());
        return ptr::null_mut();
    }

    let bundle = match BundleManager::singleton().load_bundle(&bundle_hash) {
        Ok(b) => b,
        Err(e) => {
            error!(
                "DB: Bundle {} not found in cache during FFI callback: {}",
                bundle_hash, e
            );
            return std::ptr::null_mut();
        }
    };

    let msg = build_bundle_get_by_id_message(job_id);

    debug!(
        "DB: get_job_by_id req - bundle hash: {}, jobId: {}",
        bundle_hash, job_id
    );

    let error_obj = get_bundle_db_error(&bundle_hash);
    match send_and_wait(msg) {
        Ok(response) => {
            let job_data_json = match parse_get_job_by_id_response(&response, job_id) {
                Ok(json) => json,
                Err(message) => {
                    error!(
                        "DB: get_job_by_id parse error for bundle hash: {}, jobId: {}: {}",
                        bundle_hash, job_id, message
                    );
                    return set_db_error_and_return_null(error_obj, &message);
                }
            };

            trace!("DB: get_job_by_id res - data: {}", job_data_json);

            // Create a dict from the JSON response
            let dict = bundle.json_loads(&job_data_json);
            if dict.is_null() {
                error!(
                    "DB: get_job_by_id failed to parse job data JSON for bundle hash: {}, jobId: {}",
                    bundle_hash, job_id
                );
                return set_db_error_and_return_null(error_obj, "Failed to parse job data JSON");
            }

            // Set job_id in the dict
            let value = PyLong_FromUnsignedLongLong(job_id);
            if !set_job_id_in_dict(
                dict,
                value,
                "get_job_by_id",
                &bundle_hash,
                job_id,
                error_obj,
            ) {
                return ptr::null_mut();
            }

            dict
        }
        Err(e) => {
            error!(
                "DB: get_job_by_id error for bundle hash: {}, jobId: {}: {}",
                bundle_hash, job_id, e
            );
            set_db_error_and_return_null(error_obj, &format!("DB error: {e}"))
        }
    }
}

// SAFETY: Called by Python C API with a valid `args` tuple pointer.
// All FFI calls (PyTuple_GetItem, PyLong_FromUnsignedLongLong, PyDict_SetItemString,
// etc.) operate on pointers derived from `args` or freshly created Python objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn delete_job(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let dict = PyTuple_GetItem(args, 0);
    if dict.is_null() {
        error!("DB: delete_job - invalid arguments");
        return ptr::null_mut();
    }

    let Some((bundle_hash, job_id, _)) = load_bundle_and_job_id(dict) else {
        return ptr::null_mut();
    };

    if job_id == 0 {
        warn!(
            "DB: delete_job error - no job_id provided for bundle hash: {}",
            bundle_hash
        );
        let error_obj = get_bundle_db_error(&bundle_hash);
        return set_db_error_and_return_null(error_obj, "Job ID must be provided.");
    }

    let msg = build_bundle_delete_message(job_id);

    debug!(
        "DB: delete_job req - bundle hash: {}, jobId: {}",
        bundle_hash, job_id
    );

    match send_and_wait(msg) {
        Ok(_) => {
            debug!("DB: delete_job res - success");

            return_py_none()
        }
        Err(e) => {
            error!(
                "DB: delete_job error for bundle hash: {}, jobId: {}: {}",
                bundle_hash, job_id, e
            );
            let error_obj = get_bundle_db_error(&bundle_hash);
            set_db_error_and_return_null(error_obj, &format!("DB error: {e}"))
        }
    }
}

static mut BUNDLE_DB_METHODS: [PyMethodDef; 4] = [
    PyMethodDef {
        ml_name: c"create_or_update_job".as_ptr(),
        ml_meth: Some(create_or_update_job),
        ml_flags: METH_VARARGS,
        ml_doc: c"Updates a job record in the database if one already exists, otherwise inserts the job in to the database".as_ptr(),
    },
    PyMethodDef {
        ml_name: c"get_job_by_id".as_ptr(),
        ml_meth: Some(get_job_by_id),
        ml_flags: METH_VARARGS,
        ml_doc: c"Gets a job record if one exists for the provided id".as_ptr(),
    },
    PyMethodDef {
        ml_name: c"delete_job".as_ptr(),
        ml_meth: Some(delete_job),
        ml_flags: METH_VARARGS,
        ml_doc: c"Deletes a job record from the database".as_ptr(),
    },
    PyMethodDef {
        ml_name: ptr::null(),
        ml_meth: None,
        ml_flags: 0,
        ml_doc: ptr::null(),
    },
];

static mut BUNDLE_DB_MODULE: PyModuleDef = PyModuleDef {
    m_base: PyModuleDef_Base {
        ob_base: PyObject_Head {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        },
        m_init: None,
        m_index: 0,
        m_copy: ptr::null_mut(),
    },
    m_name: c"_bundledb".as_ptr(),
    m_doc: ptr::null(),
    m_size: -1,
    m_methods: ptr::null_mut(),
    m_slots: ptr::null_mut(),
    m_traverse: ptr::null_mut(),
    m_clear: ptr::null_mut(),
    m_free: ptr::null_mut(),
};

// SAFETY: Called by Python interpreter during module import.
// Returns a new reference to the module on success, or null on error.
// All FFI calls (PyModule_Create2, PyErr_NewException, PyModule_AddObject, etc.)
// follow Python C API ownership conventions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInit_bundledb() -> *mut PyObject {
    BUNDLE_DB_MODULE.m_methods = (&raw mut BUNDLE_DB_METHODS).cast::<PyMethodDef>();

    let module = PyModule_Create2(&raw mut BUNDLE_DB_MODULE, PYTHON_API_VERSION);
    if module.is_null() {
        return ptr::null_mut();
    }

    let exc = PyErr_NewException(
        c"_bundledb.error".as_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if exc.is_null() {
        Py_DecRef(module);
        return ptr::null_mut();
    }
    Py_IncRef(exc);

    if PyModule_AddObject(module, c"error".as_ptr(), exc) < 0 {
        Py_XDECREF(exc);
        Py_DecRef(module);
        return ptr::null_mut();
    }

    // Store the exception for this bundle so callbacks can reference it.
    if let Some(bundle_hash) = get_current_thread_bundle() {
        set_bundle_db_error(&bundle_hash, exc);
    }

    module
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::DB_RESPONSE;

    #[test]
    fn err_cstring_preserves_well_formed_message() {
        let cstr = err_cstring("Job with ID 9 does not exist.");
        assert_eq!(cstr.to_str().unwrap(), "Job with ID 9 does not exist.");
    }

    #[test]
    fn err_cstring_falls_back_on_interior_nul() {
        let cstr = err_cstring("bad\x00message");
        assert_eq!(cstr.to_str().unwrap(), "Internal error");
    }

    #[test]
    fn build_bundle_create_or_update_message_matches_server_order() {
        let mut msg = Message::from_data(
            build_bundle_create_or_update_message("hash-1", 42, "{\"a\":1}")
                .get_data()
                .clone(),
        );

        assert_eq!(msg.id, DB_BUNDLE_CREATE_OR_UPDATE_JOB);
        assert_eq!(msg.pop_ulong(), 42);
        assert_eq!(msg.pop_string(), "{\"a\":1}");
        assert_eq!(msg.pop_string(), "hash-1");
    }

    #[test]
    fn build_bundle_get_by_id_message_sends_only_job_id() {
        let mut msg = Message::from_data(build_bundle_get_by_id_message(9).get_data().clone());

        assert_eq!(msg.id, DB_BUNDLE_GET_JOB_BY_ID);
        assert_eq!(msg.pop_ulong(), 9);
    }

    #[test]
    fn build_bundle_delete_message_sends_only_job_id() {
        let mut msg = Message::from_data(build_bundle_delete_message(9).get_data().clone());

        assert_eq!(msg.id, DB_BUNDLE_DELETE_JOB);
        assert_eq!(msg.pop_ulong(), 9);
    }

    #[test]
    fn parse_get_job_by_id_response_reads_count_then_payload() {
        let mut response = Message::new(DB_RESPONSE, Priority::Highest, "database");
        response.push_uint(1);
        response.push_ulong(9);
        response.push_string("{\"job_id\":9}");

        let payload = parse_get_job_by_id_response(&response, 9).unwrap();
        assert_eq!(payload, "{\"job_id\":9}");
    }

    #[test]
    fn parse_create_or_update_response_after_request_id_consumed() {
        let mut wire_response = Message::new(DB_RESPONSE, Priority::Highest, "system");
        wire_response.push_uint(42);
        wire_response.push_ulong(1234);

        let mut delivered = Message::from_data(wire_response.get_data().clone());
        assert_eq!(delivered.pop_uint(), 42);

        assert_eq!(parse_create_or_update_response(&delivered).unwrap(), 1234);
    }

    #[test]
    fn parse_get_job_by_id_response_after_request_id_consumed() {
        let mut wire_response = Message::new(DB_RESPONSE, Priority::Highest, "system");
        wire_response.push_uint(42);
        wire_response.push_uint(1);
        wire_response.push_ulong(9);
        wire_response.push_string("{\"job_id\":9}");

        let mut delivered = Message::from_data(wire_response.get_data().clone());
        assert_eq!(delivered.pop_uint(), 42);

        let payload = parse_get_job_by_id_response(&delivered, 9).unwrap();
        assert_eq!(payload, "{\"job_id\":9}");
    }

    #[test]
    fn parse_create_or_update_response_rejects_zero_job_id() {
        let mut response = Message::new(DB_RESPONSE, Priority::Highest, "database");
        response.push_ulong(0);

        let err = parse_create_or_update_response(&response).unwrap_err();
        assert_eq!(err, "Job was unable to be created or updated.");
    }

    #[test]
    fn parse_get_job_by_id_response_rejects_missing_job() {
        let mut response = Message::new(DB_RESPONSE, Priority::Highest, "database");
        response.push_uint(0);

        let err = parse_get_job_by_id_response(&response, 9).unwrap_err();
        assert_eq!(err, "Job with ID 9 does not exist.");
    }

    #[test]
    fn set_job_id_in_dict_returns_false_and_sets_error_on_null_value() {
        crate::tests::init_python_global();
        let fixture = crate::tests::fixtures::bundle_fixture::BundleFixture::new();
        let bundle_hash = "test_set_job_id_null";
        fixture.write_bundle_db_create_or_update_job(bundle_hash, r#"{"test": 1}"#);
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        let bundle = BundleManager::singleton()
            .load_bundle(bundle_hash)
            .expect("bundle should load");

        let _guard = crate::python_interface::PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let dict = crate::python_interface::PyDict_New();
            let error_obj = get_bundle_db_error(bundle_hash);
            let ok = set_job_id_in_dict(
                dict,
                ptr::null_mut(),
                "create_or_update_job",
                bundle_hash,
                42,
                error_obj,
            );
            assert!(!ok, "null value should report failure");
            assert!(
                !crate::python_interface::PyErr_Occurred().is_null(),
                "Python error should be set"
            );
            crate::python_interface::PyErr_Clear();
            crate::python_interface::Py_DecRef(dict);
        }
    }

    #[test]
    fn set_job_id_in_dict_returns_false_and_sets_error_on_dict_insertion_failure() {
        crate::tests::init_python_global();
        let fixture = crate::tests::fixtures::bundle_fixture::BundleFixture::new();
        let bundle_hash = "test_set_job_id_dict_fail";
        fixture.write_bundle_db_create_or_update_job(bundle_hash, r#"{"test": 1}"#);
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        let bundle = BundleManager::singleton()
            .load_bundle(bundle_hash)
            .expect("bundle should load");

        let _guard = crate::python_interface::PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            // A tuple is not a dict, so PyDict_SetItemString fails deterministically.
            let not_a_dict = crate::python_interface::PyTuple_New(0);
            let value = PyLong_FromUnsignedLongLong(42);
            let error_obj = get_bundle_db_error(bundle_hash);
            let ok = set_job_id_in_dict(
                not_a_dict,
                value,
                "create_or_update_job",
                bundle_hash,
                42,
                error_obj,
            );
            assert!(!ok, "failed dict insertion should report failure");
            assert!(
                !crate::python_interface::PyErr_Occurred().is_null(),
                "Python error should be set"
            );
            crate::python_interface::PyErr_Clear();
            crate::python_interface::Py_DecRef(not_a_dict);
        }
    }

    #[test]
    fn set_job_id_in_dict_sets_job_id_on_success() {
        crate::tests::init_python_global();
        let fixture = crate::tests::fixtures::bundle_fixture::BundleFixture::new();
        let bundle_hash = "test_set_job_id_success";
        fixture.write_bundle_db_create_or_update_job(bundle_hash, r#"{"test": 1}"#);
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        let bundle = BundleManager::singleton()
            .load_bundle(bundle_hash)
            .expect("bundle should load");

        let _guard = crate::python_interface::PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let dict = crate::python_interface::PyDict_New();
            let value = PyLong_FromUnsignedLongLong(42);
            let error_obj = get_bundle_db_error(bundle_hash);
            let ok = set_job_id_in_dict(dict, value, "get_job_by_id", bundle_hash, 42, error_obj);
            assert!(ok, "valid value should succeed");
            assert!(
                crate::python_interface::PyErr_Occurred().is_null(),
                "no Python error should be set"
            );
            crate::python_interface::Py_DecRef(dict);
        }
    }

    #[test]
    fn get_bundle_db_error_creates_and_caches_fallback_runtime_error() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of get_bundle_db_error (which calls PyErr_NewException).
        unsafe {
            let _guard = crate::python_interface::PYTHON_MUTEX.lock();
            let interp = (*crate::python_interface::get_main_ts()).interp;
            let _scope = crate::python_interface::ThreadScope::new(interp)
                .expect("thread scope should be created");
            let bundle_hash = "test_get_bundle_db_error_fallback";
            let first = get_bundle_db_error(bundle_hash);
            assert!(!first.is_null(), "fallback RuntimeError should be non-null");
            let second = get_bundle_db_error(bundle_hash);
            assert_eq!(
                first, second,
                "fallback exception should be cached for the bundle hash"
            );
        }
    }
}
