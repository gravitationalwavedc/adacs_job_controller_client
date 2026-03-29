use crate::python_interface::*;
use crate::messaging::{Message, Priority, DB_BUNDLE_CREATE_OR_UPDATE_JOB, DB_BUNDLE_GET_JOB_BY_ID, DB_BUNDLE_DELETE_JOB};
use crate::websocket::get_websocket_client;
use crate::thread_bundle_map::get_current_thread_bundle;
use std::ffi::{CString, CStr};
use std::os::raw::c_char;
use std::ptr;
use log::error;

static mut BUNDLE_DB_ERROR: *mut PyObject = ptr::null_mut();

pub unsafe fn py_to_json_string(obj: *mut PyObject, json_module: *mut PyObject) -> String {
    if obj.is_null() || json_module.is_null() { return "null".to_string(); }
    unsafe {
        let dumps_func = PyObject_GetAttrString(json_module, b"dumps\0".as_ptr() as *const c_char);
        if dumps_func.is_null() { return "null".to_string(); }
        let args = PyTuple_New(1);
        Py_IncRef(obj);
        PyTuple_SetItem(args, 0, obj);
        let p_result = PyObject_CallObject(dumps_func, args);
        let mut result = "null".to_string();
        if !p_result.is_null() {
            let c_str = PyUnicode_AsUTF8(p_result);
            if !c_str.is_null() {
                result = CStr::from_ptr(c_str).to_string_lossy().into_owned();
            }
            Py_DecRef(p_result);
        }
        Py_DecRef(args);
        Py_DecRef(dumps_func);
        result
    }
}

pub unsafe fn json_string_to_py(json_str: &str, json_module: *mut PyObject) -> *mut PyObject {
    if json_module.is_null() { return my_py_none_struct(); }
    unsafe {
        let loads_func = PyObject_GetAttrString(json_module, b"loads\0".as_ptr() as *const c_char);
        if loads_func.is_null() { return my_py_none_struct(); }
        let args = PyTuple_New(1);
        let p_str = PyUnicode_FromString(CString::new(json_str).unwrap().as_ptr());
        PyTuple_SetItem(args, 0, p_str);
        let result = PyObject_CallObject(loads_func, args);
        Py_DecRef(args);
        Py_DecRef(loads_func);
        if result.is_null() {
            return my_py_none_struct();
        }
        result
    }
}

unsafe extern "C" fn create_or_update_job(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let dict = unsafe { PyTuple_GetItem(args, 0) };
    if dict.is_null() { return ptr::null_mut(); }

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    
    let json_str = unsafe {
        let json_mod = PyImport_ImportModule(b"json\0".as_ptr() as *const c_char);
        let s = py_to_json_string(dict, json_mod);
        Py_XDECREF(json_mod);
        s
    };

    let mut msg = Message::new(DB_BUNDLE_CREATE_OR_UPDATE_JOB, Priority::Highest, &bundle_hash);
    msg.push_string(&json_str);

    let ws = get_websocket_client();
    
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = ws.send_db_request(msg).await;
        let _ = tx.send(res);
    });

    // Timeout after 5 seconds to avoid permanent hang
    let response_res = rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())
        .and_then(|res| res.map_err(|e| e.to_string()));

    match response_res {
        Ok(mut resp) => {
            let success = resp.pop_bool();
            if !success {
                let err_text = resp.pop_string();
                unsafe {
                    let err_msg = CString::new(format!("WebSocket error: {}", err_text)).unwrap();
                    PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                }
                return ptr::null_mut();
            }
            let resp_json = resp.pop_string();
            unsafe {
                let json_mod = PyImport_ImportModule(b"json\0".as_ptr() as *const c_char);
                let obj = json_string_to_py(&resp_json, json_mod);
                Py_XDECREF(json_mod);
                obj
            }
        }
        Err(e) => {
            error!("WS Error in create_or_update_job: {}", e);
            unsafe { 
                let err_msg = CString::new(format!("WebSocket error: {}", e)).unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
            }
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn get_job_by_id(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let job_id_obj = unsafe { PyTuple_GetItem(args, 0) };
    if job_id_obj.is_null() { return ptr::null_mut(); }
    let job_id = unsafe { PyLong_AsUnsignedLongLong(job_id_obj) };

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    
    let mut msg = Message::new(DB_BUNDLE_GET_JOB_BY_ID, Priority::Highest, &bundle_hash);
    msg.push_ulong(job_id);

    let ws = get_websocket_client();
    
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = ws.send_db_request(msg).await;
        let _ = tx.send(res);
    });

    let response_res = rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())
        .and_then(|res| res.map_err(|e| e.to_string()));

    match response_res {
        Ok(mut resp) => {
            let success = resp.pop_bool();
            if !success {
                let err_text = resp.pop_string();
                unsafe {
                    let err_msg = CString::new(format!("WebSocket error: {}", err_text)).unwrap();
                    PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                }
                return ptr::null_mut();
            }
            let resp_json = resp.pop_string();
            unsafe {
                let json_mod = PyImport_ImportModule(b"json\0".as_ptr() as *const c_char);
                let obj = json_string_to_py(&resp_json, json_mod);
                Py_XDECREF(json_mod);
                obj
            }
        }
        Err(e) => {
            error!("WS Error in get_job_by_id: {}", e);
            unsafe { 
                let err_msg = CString::new(format!("WebSocket error: {}", e)).unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
            }
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn delete_job(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let dict = unsafe { PyTuple_GetItem(args, 0) };
    if dict.is_null() { return ptr::null_mut(); }

    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());
    let json_str = unsafe {
        let json_mod = PyImport_ImportModule(b"json\0".as_ptr() as *const c_char);
        let s = py_to_json_string(dict, json_mod);
        Py_XDECREF(json_mod);
        s
    };

    let mut msg = Message::new(DB_BUNDLE_DELETE_JOB, Priority::Highest, &bundle_hash);
    msg.push_string(&json_str);

    let ws = get_websocket_client();
    
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = ws.send_db_request(msg).await;
        let _ = tx.send(res);
    });

    let response_res = rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())
        .and_then(|res| res.map_err(|e| e.to_string()));

    match response_res {
        Ok(mut resp) => {
            let success = resp.pop_bool();
            if success {
                let none = unsafe { my_py_none_struct() };
                unsafe { Py_IncRef(none) };
                none
            } else {
                let err_text = resp.pop_string();
                unsafe {
                    let err_msg = CString::new(format!("WebSocket error: {}", err_text)).unwrap();
                    PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
                }
                ptr::null_mut()
            }
        }
        Err(e) => {
            error!("WS Error in delete_job: {}", e);
            unsafe { 
                let err_msg = CString::new(format!("WebSocket error: {}", e)).unwrap();
                PyErr_SetString(BUNDLE_DB_ERROR, err_msg.as_ptr());
            }
            ptr::null_mut()
        }
    }
}

static mut METHODS: [PyMethodDef; 4] = [
    PyMethodDef {
        ml_name: ptr::null(),
        ml_meth: Some(create_or_update_job),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    },
    PyMethodDef {
        ml_name: ptr::null(),
        ml_meth: Some(get_job_by_id),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    },
    PyMethodDef {
        ml_name: ptr::null(),
        ml_meth: Some(delete_job),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    },
    PyMethodDef {
        ml_name: ptr::null(),
        ml_meth: None,
        ml_flags: 0,
        ml_doc: ptr::null(),
    },
];

static mut MODULE_DEF: PyModuleDef = PyModuleDef {
    m_base: PyModuleDef_Base {
        ob_base: PyObject_Head {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        },
        m_init: None,
        m_index: 0,
        m_copy: ptr::null_mut(),
    },
    m_name: ptr::null(),
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
    let s_create_or_update_job = CString::new("create_or_update_job").unwrap();
    let s_get_job_by_id = CString::new("get_job_by_id").unwrap();
    let s_delete_job = CString::new("delete_job").unwrap();
    let s_create_or_update_job_doc = CString::new("Updates a job record in the database if one already exists, otherwise inserts the job in to the database").unwrap();
    let s_get_job_by_id_doc = CString::new("Gets a job record if one exists for the provided id").unwrap();
    let s_delete_job_doc = CString::new("Deletes a job record from the database").unwrap();

    unsafe {
        METHODS[0].ml_name = s_create_or_update_job.into_raw();
        METHODS[0].ml_doc = s_create_or_update_job_doc.into_raw();
        METHODS[1].ml_name = s_get_job_by_id.into_raw();
        METHODS[1].ml_doc = s_get_job_by_id_doc.into_raw();
        METHODS[2].ml_name = s_delete_job.into_raw();
        METHODS[2].ml_doc = s_delete_job_doc.into_raw();

        let s_name = CString::new("_bundledb").unwrap();
        MODULE_DEF.m_name = s_name.into_raw();
        MODULE_DEF.m_methods = ptr::addr_of_mut!(METHODS) as *mut PyMethodDef;

        let module = PyModule_Create2(&raw mut MODULE_DEF, PYTHON_API_VERSION);
        if module.is_null() {
            return ptr::null_mut();
        }

        let s_error_name = CString::new("_bundledb.error").unwrap();
        BUNDLE_DB_ERROR = PyErr_NewException(s_error_name.as_ptr(), ptr::null_mut(), ptr::null_mut());
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
