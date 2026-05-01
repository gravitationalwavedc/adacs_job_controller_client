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
use std::ffi::CString;
use std::ptr;

static mut BUNDLE_DB_ERROR: *mut PyObject = ptr::null_mut();

/// Bridge from synchronous Python callback to async WebSocket.
///
/// Because this is called from a sync context (Python C callback) that may be
/// running inside a `current_thread` tokio runtime, we cannot simply spawn a
/// task and block-wait on the current thread — that would deadlock the runtime.
/// Instead we spawn a dedicated OS thread with its own single-threaded runtime
/// to drive the future to completion, then join the thread to get the result.
unsafe fn send_and_wait(msg: Message) -> Result<Message, String> {
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
    Message::from_data(response.get_data().clone())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn create_or_update_job(
    _self: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    let dict = PyTuple_GetItem(args, 0);

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    let bundle = BundleManager::singleton().load_bundle(&bundle_hash);

    // Convert first argument to a json object
    let json_str = bundle.json_dumps(dict);
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

    let mut msg = Message::new(DB_BUNDLE_CREATE_OR_UPDATE_JOB, Priority::Medium, "database");
    msg.push_string(&bundle_hash);
    msg.push_ulong(job_id);
    msg.push_string(&serde_json::to_string(&job_data_clean).unwrap_or_default());

    tracing::info!(
        "DB: create_or_update_job req - bundle hash: {}, jobId: {}",
        bundle_hash,
        job_id
    );

    match send_and_wait(msg) {
        Ok(response) => {
            let mut resp = parse_response(&response);
            let success = resp.pop_bool();

            if !success {
                PyErr_SetString(
                    BUNDLE_DB_ERROR,
                    c"Job was unable to be created or updated.".as_ptr(),
                );
                return ptr::null_mut();
            }

            let new_job_id = resp.pop_ulong();

            // Set job_id in the original dict (matches C++ exactly)
            let value = PyLong_FromUnsignedLongLong(new_job_id);
            PyDict_SetItemString(dict, c"job_id".as_ptr(), value);
            Py_DecRef(value);
            Py_IncRef(dict);

            tracing::info!("DB: create_or_update_job res - jobId: {}", new_job_id);

            let result = my_py_none_struct();
            Py_IncRef(result);
            result
        }
        Err(e) => {
            let err_msg = CString::new(format!("DB error: {e}")).unwrap();
            PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn get_job_by_id(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let job_id_obj = PyTuple_GetItem(args, 0);
    let job_id = PyLong_AsUnsignedLongLong(job_id_obj);

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    let bundle = BundleManager::singleton().load_bundle(&bundle_hash);

    let mut msg = Message::new(DB_BUNDLE_GET_JOB_BY_ID, Priority::Medium, "database");
    msg.push_string(&bundle_hash);
    msg.push_ulong(job_id);

    tracing::info!(
        "DB: get_job_by_id req - bundle hash: {}, jobId: {}",
        bundle_hash,
        job_id
    );

    match send_and_wait(msg) {
        Ok(response) => {
            let mut resp = parse_response(&response);
            let success = resp.pop_bool();

            if !success {
                let err_msg =
                    CString::new(format!("Job with ID {job_id} does not exist.")).unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                return ptr::null_mut();
            }

            // We can ignore the jobId in response (matches C++)
            let _resp_job_id = resp.pop_ulong();
            let job_data_json = resp.pop_string();

            tracing::info!("DB: get_job_by_id res - data: {}", job_data_json);

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
            let err_msg = CString::new(format!("DB error: {e}")).unwrap();
            PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn delete_job(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let dict = PyTuple_GetItem(args, 0);

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    let bundle = BundleManager::singleton().load_bundle(&bundle_hash);

    let json_str = bundle.json_dumps(dict);
    let job_data: serde_json::Value =
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null);

    let job_id = job_data
        .get("job_id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    if job_id == 0 {
        PyErr_SetString(BUNDLE_DB_ERROR, c"Job ID must be provided.".as_ptr());
        return ptr::null_mut();
    }

    let mut msg = Message::new(DB_BUNDLE_DELETE_JOB, Priority::Medium, "database");
    msg.push_string(&bundle_hash);
    msg.push_ulong(job_id);

    tracing::info!(
        "DB: delete_job req - bundle hash: {}, jobId: {}",
        bundle_hash,
        job_id
    );

    match send_and_wait(msg) {
        Ok(response) => {
            let mut resp = parse_response(&response);
            let success = resp.pop_bool();

            if !success {
                let err_msg =
                    CString::new(format!("Job with ID {job_id} does not exist.")).unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                return ptr::null_mut();
            }

            tracing::info!("DB: delete_job res - success");

            let result = my_py_none_struct();
            Py_IncRef(result);
            result
        }
        Err(e) => {
            let err_msg = CString::new(format!("DB error: {e}")).unwrap();
            PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInit_bundledb() -> *mut PyObject {
    BUNDLE_DB_MODULE.m_methods = (&raw mut BUNDLE_DB_METHODS).cast::<PyMethodDef>();

    let module = PyModule_Create2(&raw mut BUNDLE_DB_MODULE, PYTHON_API_VERSION);
    if module.is_null() {
        return ptr::null_mut();
    }

    BUNDLE_DB_ERROR = PyErr_NewException(
        c"_bundledb.error".as_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    Py_IncRef(BUNDLE_DB_ERROR);

    if PyModule_AddObject(module, c"error".as_ptr(), BUNDLE_DB_ERROR) < 0 {
        Py_XDECREF(BUNDLE_DB_ERROR);
        BUNDLE_DB_ERROR = ptr::null_mut();
        Py_DecRef(module);
        return ptr::null_mut();
    }

    module
}
