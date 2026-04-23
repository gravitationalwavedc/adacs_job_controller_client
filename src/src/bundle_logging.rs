//! Port of C++ BundleLogging.
//!
//! Provides the `_bundlelogging` Python module with a single `write` method.
//! When Python code calls `print()`, the stdout/stderr catchers (installed by
//! the BundleInterface constructor) call `_bundlelogging.write(is_stdout, msg)`.
//!
//! The C++ version uses a global `std::vector<std::string> lineParts`.
//! We use thread-local storage for safety.

use crate::python_interface::*;
use crate::thread_bundle_map::get_current_thread_bundle;
use std::ffi::{c_char, CStr};

thread_local! {
    static LINE_PARTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    static LAST_MESSAGE: std::cell::RefCell<Option<(String, bool)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn get_last_log_message() -> Option<(String, bool)> {
    LAST_MESSAGE.with(|last| last.borrow().clone())
}

/// The Python-callable `write(is_stdout, message)` function.
/// Mirrors C++ `writeLog` exactly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn write_log(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    // Get the bundle hash for this thread
    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());

    // Convert first argument to a bool (is_stdout)
    let arg0 = PyTuple_GetItem(args, 0);
    let is_stdout = arg0 == my_py_true_struct();

    // Convert the second argument to a string
    let arg1 = PyTuple_GetItem(args, 1);
    let c_msg = PyUnicode_AsUTF8(arg1);

    if c_msg.is_null() {
        // Return Py_None with INCREF like C++
        let result = my_py_none_struct();
        Py_IncRef(result);
        return result;
    }

    let msg = CStr::from_ptr(c_msg).to_string_lossy();

    // Don't write trailing newlines – accumulate line parts
    if msg != "\n" {
        LINE_PARTS.with(|parts| {
            parts.borrow_mut().push(msg.to_string());
        });
    } else {
        #[cfg(test)]
        let _ = is_stdout; // suppress unused warning in cfg block below

        let mut full_message = format!("Bundle [{}]: ", bundle_hash);
        LINE_PARTS.with(|parts| {
            let mut parts = parts.borrow_mut();
            for part in parts.iter() {
                full_message.push_str(part);
            }
            parts.clear();
        });

        #[cfg(test)]
        LAST_MESSAGE.with(|last| {
            *last.borrow_mut() = Some((full_message.clone(), is_stdout));
        });

        if is_stdout {
            tracing::info!("{}", full_message);
        } else {
            tracing::error!("{}", full_message);
        }
    }

    // Return Py_None with INCREF (matches C++ exactly)
    let result = my_py_none_struct();
    Py_IncRef(result);
    result
}

static mut BUNDLE_LOGGING_METHODS: [PyMethodDef; 2] = [
    PyMethodDef {
        ml_name: b"write\0".as_ptr() as *const c_char,
        ml_meth: Some(write_log),
        ml_flags: METH_VARARGS,
        ml_doc: b"Writes the provided message to the client log file.\0".as_ptr() as *const c_char,
    },
    PyMethodDef {
        ml_name: std::ptr::null(),
        ml_meth: None,
        ml_flags: 0,
        ml_doc: std::ptr::null(),
    },
];

static mut BUNDLE_LOGGING_MODULE: PyModuleDef = PyModuleDef {
    m_base: PyModuleDef_Base {
        ob_base: PyObject_Head {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        },
        m_init: None,
        m_index: 0,
        m_copy: std::ptr::null_mut(),
    },
    m_name: b"_bundlelogging\0".as_ptr() as *const c_char,
    m_doc: std::ptr::null(),
    m_size: -1,
    m_methods: std::ptr::null_mut(),
    m_slots: std::ptr::null_mut(),
    m_traverse: std::ptr::null_mut(),
    m_clear: std::ptr::null_mut(),
    m_free: std::ptr::null_mut(),
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyInit_bundlelogging() -> *mut PyObject {
    unsafe {
        BUNDLE_LOGGING_MODULE.m_methods = (&raw mut BUNDLE_LOGGING_METHODS) as *mut PyMethodDef;
        PyModule_Create2(&raw mut BUNDLE_LOGGING_MODULE, PYTHON_API_VERSION)
    }
}
