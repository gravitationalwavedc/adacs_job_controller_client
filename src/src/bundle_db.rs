//! Port of C++ BundleDB.
//!
//! Provides the `_bundledb` Python module with create_or_update_job, get_job_by_id,
//! and delete_job methods that communicate with the server via WebSocket.
//!
//! Each method:
//!  1. Gets the bundle hash from the thread bundle map
//!  2. Loads the bundle from BundleManager (always a cache hit)
//!  3. Builds a Message matching the C++ wire protocol
//!  4. Sends via the async WebSocket client using a sync→async bridge
//!  5. Processes the response and returns appropriate Python objects

use crate::bundle_manager::BundleManager;
use crate::messaging::{
    Message, Priority, DB_BUNDLE_CREATE_OR_UPDATE_JOB, DB_BUNDLE_DELETE_JOB,
    DB_BUNDLE_GET_JOB_BY_ID,
};
use crate::python_interface::*;
use crate::thread_bundle_map::get_current_thread_bundle;
use crate::websocket::get_websocket_client;
use std::ffi::{c_char, CString};
use std::ptr;

static mut BUNDLE_DB_ERROR: *mut PyObject = ptr::null_mut();

/// Bridge from synchronous Python callback to async WebSocket.
/// Spawns the async send on the current tokio runtime and blocks waiting
/// for the result via an mpsc channel.
unsafe fn send_and_wait(msg: Message) -> Result<Message, String> {
    let ws = get_websocket_client();
    let (tx, rx) = std::sync::mpsc::channel();

    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "No tokio runtime available".to_string())?;

    handle.spawn(async move {
        let res = ws.send_db_request(msg).await;
        let _ = tx.send(res);
    });

    rx.recv_timeout(std::time::Duration::from_secs(30))
        .map_err(|e| format!("Timeout waiting for DB response: {}", e))?
        .map_err(|e| format!("DB request error: {}", e))
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
    let job_id = job_data.get("job_id").and_then(|v| v.as_u64()).unwrap_or(0);

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
                let err_msg = CString::new("Job was unable to be created or updated.").unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                return ptr::null_mut();
            }

            let new_job_id = resp.pop_ulong();

            // Set job_id in the original dict (matches C++ exactly)
            let value = PyLong_FromUnsignedLongLong(new_job_id);
            let key = CString::new("job_id").unwrap();
            PyDict_SetItemString(dict, key.as_ptr(), value);
            Py_DecRef(value);
            Py_IncRef(dict);

            tracing::info!("DB: create_or_update_job res - jobId: {}", new_job_id);

            let result = my_py_none_struct();
            Py_IncRef(result);
            result
        }
        Err(e) => {
            let err_msg = CString::new(format!("DB error: {}", e)).unwrap();
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
                    CString::new(format!("Job with ID {} does not exist.", job_id)).unwrap();
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
            let key = CString::new("job_id").unwrap();
            PyDict_SetItemString(dict, key.as_ptr(), value);
            Py_DecRef(value);
            Py_IncRef(dict);

            dict
        }
        Err(e) => {
            let err_msg = CString::new(format!("DB error: {}", e)).unwrap();
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

    let job_id = job_data.get("job_id").and_then(|v| v.as_u64()).unwrap_or(0);

    if job_id == 0 {
        let err_msg = CString::new("Job ID must be provided.").unwrap();
        unsafe {
            PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
        }
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
                    CString::new(format!("Job with ID {} does not exist.", job_id)).unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                return ptr::null_mut();
            }

            tracing::info!("DB: delete_job res - success");

            let result = my_py_none_struct();
            Py_IncRef(result);
            result
        }
        Err(e) => {
            let err_msg = CString::new(format!("DB error: {}", e)).unwrap();
            PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
            ptr::null_mut()
        }
    }
}

static mut BUNDLE_DB_METHODS: [PyMethodDef; 4] = [
    PyMethodDef {
        ml_name: b"create_or_update_job\0".as_ptr() as *const c_char,
        ml_meth: Some(create_or_update_job),
        ml_flags: METH_VARARGS,
        ml_doc: b"Updates a job record in the database if one already exists, otherwise inserts the job in to the database\0".as_ptr() as *const c_char,
    },
    PyMethodDef {
        ml_name: b"get_job_by_id\0".as_ptr() as *const c_char,
        ml_meth: Some(get_job_by_id),
        ml_flags: METH_VARARGS,
        ml_doc: b"Gets a job record if one exists for the provided id\0".as_ptr() as *const c_char,
    },
    PyMethodDef {
        ml_name: b"delete_job\0".as_ptr() as *const c_char,
        ml_meth: Some(delete_job),
        ml_flags: METH_VARARGS,
        ml_doc: b"Deletes a job record from the database\0".as_ptr() as *const c_char,
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
    m_name: b"_bundledb\0".as_ptr() as *const c_char,
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
    unsafe {
        BUNDLE_DB_MODULE.m_methods = (&raw mut BUNDLE_DB_METHODS) as *mut PyMethodDef;

        let module = PyModule_Create2(&raw mut BUNDLE_DB_MODULE, PYTHON_API_VERSION);
        if module.is_null() {
            return ptr::null_mut();
        }

        let s_error_name = CString::new("_bundledb.error").unwrap();
        BUNDLE_DB_ERROR =
            PyErr_NewException(s_error_name.as_ptr(), ptr::null_mut(), ptr::null_mut());
        Py_IncRef(BUNDLE_DB_ERROR);

        let s_error = CString::new("error").unwrap();
        if PyModule_AddObject(module, s_error.as_ptr(), BUNDLE_DB_ERROR) < 0 {
            Py_XDECREF(BUNDLE_DB_ERROR);
            BUNDLE_DB_ERROR = ptr::null_mut();
            Py_DecRef(module);
            return ptr::null_mut();
        }

        module
    }
}
