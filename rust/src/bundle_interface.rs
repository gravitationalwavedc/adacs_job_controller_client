use crate::python_interface::*;
use crate::bundle_logging::PyInit_bundlelogging;
use std::ffi::{CString, CStr};
use std::os::raw::c_char;
use serde_json::Value;
use log::error;
use std::sync::Arc;

struct BundleInterfaceInner {
    interp: *mut PyInterpreterState,
    p_module: *mut PyObject,
    p_json_module: *mut PyObject,
}

#[derive(Clone)]
pub struct BundleInterface {
    inner: Arc<BundleInterfaceInner>,
}

unsafe impl Send for BundleInterface {}
unsafe impl Sync for BundleInterface {}

impl BundleInterface {
    pub unsafe fn new(module_name: &str, script_path: &str) -> Self {
        let _guard = PYTHON_MUTEX.lock();
        crate::python_interface::init_python();

        // Standard sub-interpreter creation sequence
        let main_ts = PyThreadState_Get();
        
        let ts_new = Py_NewInterpreter();
        if ts_new.is_null() {
            panic!("Failed to create new interpreter");
        }
        let interp = (*ts_new).interp;
        
        // At this point, ts_new is the current thread state for the new interpreter
        
        // Initialize _bundlelogging in this sub-interpreter
        let s_bundlelogging = CString::new("_bundlelogging").unwrap();
        let p_logging_module = PyInit_bundlelogging();
        let sys_modules = PyImport_GetModuleDict();
        PyDict_SetItem(sys_modules, s_bundlelogging.as_ptr(), p_logging_module);
        Py_DecRef(p_logging_module);

        // Redirect stdout/stderr
        let s_sys = CString::new("sys").unwrap();
        let sys_module = PyImport_ImportModule(s_sys.as_ptr());
        let bundlelogging_module = PyImport_ImportModule(s_bundlelogging.as_ptr());
        PyObject_SetAttrString(sys_module, b"stdout\0".as_ptr() as *const c_char, bundlelogging_module);
        PyObject_SetAttrString(sys_module, b"stderr\0".as_ptr() as *const c_char, bundlelogging_module);
        Py_DecRef(sys_module);
        Py_DecRef(bundlelogging_module);

        let s_json = CString::new("json").unwrap();
        let p_json_module = PyImport_ImportModule(s_json.as_ptr());
        
        let sys_path = PySys_GetObject(b"path\0".as_ptr() as *const c_char);
        let p_path = PyUnicode_FromString(CString::new(script_path).unwrap().as_ptr());
        PyList_Append(sys_path, p_path);
        Py_DecRef(p_path);

        let s_name = CString::new(module_name).unwrap();
        let p_module = PyImport_ImportModule(s_name.as_ptr());
        if p_module.is_null() {
            error!("Failed to load python bundle module: {}", module_name);
            PyErr_Print();
        }

        // Swap back to main thread state (which releases the new interpreter's state)
        PyThreadState_Swap(main_ts);

        Self {
            inner: Arc::new(BundleInterfaceInner {
                interp,
                p_module,
                p_json_module,
            }),
        }
    }

    pub unsafe fn run(&self, func: &str, details: &Value, job_data: &str) -> *mut PyObject {
        let _guard = PYTHON_MUTEX.lock();
        let ts = PyThreadState_New(self.inner.interp);
        let old_ts = PyThreadState_Swap(ts);

        let mut p_result = std::ptr::null_mut();

        if !self.inner.p_module.is_null() {
            let s_func = CString::new(func).unwrap();
            let p_func = PyObject_GetAttrString(self.inner.p_module, s_func.as_ptr());

            if !p_func.is_null() && PyCallable_Check(p_func) != 0 {
                let json_str = serde_json::to_string(details).unwrap();
                let p_details = crate::bundle_db::json_string_to_py(&json_str, self.inner.p_json_module);
                let p_job_data = PyUnicode_FromString(CString::new(job_data).unwrap().as_ptr());
                
                let p_args_tuple = PyTuple_New(2);
                PyTuple_SetItem(p_args_tuple, 0, p_details);
                PyTuple_SetItem(p_args_tuple, 1, p_job_data);

                p_result = PyObject_CallObject(p_func, p_args_tuple);
                Py_DecRef(p_args_tuple);

                if p_result.is_null() {
                    PyErr_Print();
                }
            }
            Py_XDECREF(p_func);
        }

        PyThreadState_Swap(old_ts);
        PyThreadState_Clear(ts);
        PyThreadState_Delete(ts);

        p_result
    }

    pub unsafe fn to_string(&self, obj: *mut PyObject) -> String {
        if obj.is_null() { return String::new(); }
        let _guard = PYTHON_MUTEX.lock();
        let ts = PyThreadState_New(self.inner.interp);
        let old_ts = PyThreadState_Swap(ts);
        
        let p_str = PyObject_Str(obj);
        let mut result = String::new();
        if !p_str.is_null() {
            let c_str = PyUnicode_AsUTF8(p_str);
            if !c_str.is_null() {
                result = CStr::from_ptr(c_str).to_string_lossy().into_owned();
            }
            Py_DecRef(p_str);
        }

        PyThreadState_Swap(old_ts);
        PyThreadState_Clear(ts);
        PyThreadState_Delete(ts);
        result
    }

    pub unsafe fn to_uint64(&self, obj: *mut PyObject) -> u64 {
        if obj.is_null() { return 0; }
        let _guard = PYTHON_MUTEX.lock();
        let ts = PyThreadState_New(self.inner.interp);
        let old_ts = PyThreadState_Swap(ts);
        let res = PyLong_AsUnsignedLongLong(obj);
        PyThreadState_Swap(old_ts);
        PyThreadState_Clear(ts);
        PyThreadState_Delete(ts);
        res
    }

    pub unsafe fn to_bool(&self, obj: *mut PyObject) -> bool {
        if obj.is_null() { return false; }
        let _guard = PYTHON_MUTEX.lock();
        let ts = PyThreadState_New(self.inner.interp);
        let old_ts = PyThreadState_Swap(ts);
        let res = PyObject_IsTrue(obj) != 0;
        PyThreadState_Swap(old_ts);
        PyThreadState_Clear(ts);
        PyThreadState_Delete(ts);
        res
    }

    pub unsafe fn json_dumps(&self, obj: *mut PyObject) -> String {
        if obj.is_null() { return "null".to_string(); }
        let _guard = PYTHON_MUTEX.lock();
        let ts = PyThreadState_New(self.inner.interp);
        let old_ts = PyThreadState_Swap(ts);
        let res = crate::bundle_db::py_to_json_string(obj, self.inner.p_json_module);
        PyThreadState_Swap(old_ts);
        PyThreadState_Clear(ts);
        PyThreadState_Delete(ts);
        res
    }

    pub unsafe fn run_json(&self, func: &str, args: &Value) -> Value {
        let res_obj = self.run(func, args, "");
        if res_obj.is_null() { return Value::Null; }
        let json_str = self.json_dumps(res_obj);
        self.dispose_object(res_obj);
        serde_json::from_str(&json_str).unwrap_or(Value::Null)
    }

    pub unsafe fn run_string(&self, func: &str, args: &Value, source: &str) -> String {
        let res_obj = self.run(func, args, source);
        if res_obj.is_null() { return String::new(); }
        let res = self.to_string(res_obj);
        self.dispose_object(res_obj);
        res
    }

    pub unsafe fn dispose_object(&self, obj: *mut PyObject) {
        if !obj.is_null() {
            let _guard = PYTHON_MUTEX.lock();
            let ts = PyThreadState_New(self.inner.interp);
            let old_ts = PyThreadState_Swap(ts);
            Py_DecRef(obj);
            PyThreadState_Swap(old_ts);
            PyThreadState_Clear(ts);
            PyThreadState_Delete(ts);
        }
    }
}

impl Drop for BundleInterfaceInner {
    fn drop(&mut self) {
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let ts = PyThreadState_New(self.interp);
            let old_ts = PyThreadState_Swap(ts);
            Py_XDECREF(self.p_module);
            Py_XDECREF(self.p_json_module);
            Py_EndInterpreter(ts);
            PyThreadState_Swap(old_ts);
        }
    }
}
