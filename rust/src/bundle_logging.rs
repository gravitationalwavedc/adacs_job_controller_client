use std::ffi::{c_char, CStr};
use crate::python_interface::*;
use crate::thread_bundle_map::get_current_thread_bundle;

thread_local! {
    static LINE_PARTS: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    #[cfg(test)]
    static LAST_MESSAGE: std::cell::RefCell<Option<(String, bool)>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub fn get_last_log_message() -> Option<(String, bool)> {
    LAST_MESSAGE.with(|last| last.borrow().clone())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn write_log(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    let bundle_hash = get_current_thread_bundle().unwrap_or_else(|| "unknown".to_string());

    let (_arg0, is_stdout, _arg1, c_msg) = unsafe {
        let arg0 = PyTuple_GetItem(args, 0);
        let is_stdout = crate::python_interface::PyObject_IsTrue(arg0) == 1;
        let arg1 = PyTuple_GetItem(args, 1);
        let c_msg = PyUnicode_AsUTF8(arg1);
        (arg0, is_stdout, arg1, c_msg)
    };

    if c_msg.is_null() {
        return unsafe { crate::python_interface::PyLong_FromUnsignedLongLong(0) };
    }
    let msg = unsafe { CStr::from_ptr(c_msg).to_string_lossy() };
    let msg_len = msg.len() as u64;

    if msg != "\n" {
        LINE_PARTS.with(|parts| {
            parts.borrow_mut().push(msg.to_string());
        });
    } else {
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
            log::info!("{}", full_message);
        } else {
            log::error!("{}", full_message);
        }
    }

    unsafe {
        crate::python_interface::PyLong_FromUnsignedLongLong(msg_len)
    }
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
        (*(&raw mut BUNDLE_LOGGING_MODULE)).m_methods = (&raw mut BUNDLE_LOGGING_METHODS) as *mut PyMethodDef;
        PyModule_Create2(&raw mut BUNDLE_LOGGING_MODULE, PYTHON_API_VERSION)
    }
}
