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
    my_py_none_struct, PyDict_SetItemString, PyErr_NewException, PyErr_SetString,
    PyLong_AsUnsignedLongLong, PyLong_FromUnsignedLongLong, PyMethodDef, PyModuleDef,
    PyModuleDef_Base, PyModule_AddObject, PyModule_Create2, PyObject, PyObject_Head,
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
    let errors = BUNDLE_DB_ERRORS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    errors.get(bundle_hash).map_or_else(
        || {
            // Fallback: create a generic RuntimeError if no exception was stored
            // for this bundle hash. This should not happen in normal operation.
            // SAFETY: PyErr_NewException is called with a valid C string pointer
            // and null parent/base dicts, which is always safe.
            unsafe {
                let err = crate::python_interface::PyErr_NewException(
                    c"_bundledb.error".as_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                if err.is_null() {
                    ptr::null_mut()
                } else {
                    err
                }
            }
        },
        |e| e.0,
    )
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

// SAFETY: Called by Python C API with a valid `args` tuple pointer.
// All FFI calls (PyTuple_GetItem, PyDict_SetItemString, PyLong_*, etc.)
// operate on pointers derived from `args` or freshly created Python objects.
#[unsafe(no_mangle)]
unsafe extern "C" fn create_or_update_job(
    _self: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    let dict = PyTuple_GetItem(args, 0);

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
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

    let Ok(json_str) = bundle.json_dumps(dict) else {
        return std::ptr::null_mut();
    };
    let job_data: serde_json::Value =
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null);

    // Try to get the job_id from the job data
    let job_id = job_data
        .get("job_id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

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
                    let err_msg = CString::new(message).unwrap();
                    PyErr_SetString(error_obj, err_msg.as_ptr());
                    return ptr::null_mut();
                }
            };

            // Set job_id in the original dict (matches C++ exactly)
            let value = PyLong_FromUnsignedLongLong(new_job_id);
            PyDict_SetItemString(dict, c"job_id".as_ptr(), value);
            Py_DecRef(value);
            Py_IncRef(dict);

            debug!("DB: create_or_update_job res - jobId: {}", new_job_id);

            let result = my_py_none_struct();
            Py_IncRef(result);
            result
        }
        Err(e) => {
            error!(
                "DB: create_or_update_job error for bundle hash: {}, jobId: {}: {}",
                bundle_hash, job_id, e
            );
            let err_msg = CString::new(format!("DB error: {e}")).unwrap();
            PyErr_SetString(error_obj, err_msg.as_ptr());
            ptr::null_mut()
        }
    }
}

// SAFETY: Called by Python C API with a valid `args` tuple pointer.
// All FFI calls (PyTuple_GetItem, PyLong_AsUnsignedLongLong, PyDict_SetItemString,
// etc.) operate on pointers derived from `args` or freshly created Python objects.
#[unsafe(no_mangle)]
unsafe extern "C" fn get_job_by_id(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let job_id_obj = PyTuple_GetItem(args, 0);
    let job_id = PyLong_AsUnsignedLongLong(job_id_obj);

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
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
                    let err_msg = CString::new(message).unwrap();
                    PyErr_SetString(error_obj, err_msg.as_ptr());
                    return ptr::null_mut();
                }
            };

            trace!("DB: get_job_by_id res - data: {}", job_data_json);

            // Create a dict from the JSON response
            let dict = bundle.json_loads(&job_data_json);

            // Set job_id in the dict
            let value = PyLong_FromUnsignedLongLong(job_id);
            PyDict_SetItemString(dict, c"job_id".as_ptr(), value);
            Py_DecRef(value);
            Py_IncRef(dict);

            dict
        }
        Err(e) => {
            error!(
                "DB: get_job_by_id error for bundle hash: {}, jobId: {}: {}",
                bundle_hash, job_id, e
            );
            let err_msg = CString::new(format!("DB error: {e}")).unwrap();
            PyErr_SetString(error_obj, err_msg.as_ptr());
            ptr::null_mut()
        }
    }
}

// SAFETY: Called by Python C API with a valid `args` tuple pointer.
// All FFI calls (PyTuple_GetItem, PyLong_FromUnsignedLongLong, PyDict_SetItemString,
// etc.) operate on pointers derived from `args` or freshly created Python objects.
#[unsafe(no_mangle)]
unsafe extern "C" fn delete_job(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let dict = PyTuple_GetItem(args, 0);

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
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

    let Ok(json_str) = bundle.json_dumps(dict) else {
        return std::ptr::null_mut();
    };
    let job_data: serde_json::Value =
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null);

    let job_id = job_data
        .get("job_id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    if job_id == 0 {
        warn!(
            "DB: delete_job error - no job_id provided for bundle hash: {}",
            bundle_hash
        );
        let error_obj = get_bundle_db_error(&bundle_hash);
        PyErr_SetString(error_obj, c"Job ID must be provided.".as_ptr());
        return ptr::null_mut();
    }

    let msg = build_bundle_delete_message(job_id);

    debug!(
        "DB: delete_job req - bundle hash: {}, jobId: {}",
        bundle_hash, job_id
    );

    match send_and_wait(msg) {
        Ok(_) => {
            debug!("DB: delete_job res - success");

            let result = my_py_none_struct();
            Py_IncRef(result);
            result
        }
        Err(e) => {
            error!(
                "DB: delete_job error for bundle hash: {}, jobId: {}: {}",
                bundle_hash, job_id, e
            );
            let err_msg = CString::new(format!("DB error: {e}")).unwrap();
            let error_obj = get_bundle_db_error(&bundle_hash);
            PyErr_SetString(error_obj, err_msg.as_ptr());
            ptr::null_mut()
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
}
