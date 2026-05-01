//! Port of C++ `BundleInterface`.
//!
//! Each `BundleInterface` owns a `SubInterpreter` and caches the loaded bundle
//! module plus json/traceback helpers.  The C++ version never creates
//! temporary thread-states for every call – instead it creates a `ThreadScope`
//! that lives for the duration of the call.  We replicate that here.

use crate::python_interface::{
    my_py_true_struct, MyPy_IsNone, PyCallable_Check, PyDict_New, PyDict_SetItemString,
    PyErr_Fetch, PyErr_Occurred, PyErr_Print, PyEval_GetBuiltins, PyEval_RestoreThread,
    PyEval_SaveThread, PyImport_ImportModule, PyIter_Next, PyList_Append,
    PyLong_AsUnsignedLongLong, PyObject, PyObject_CallObject, PyObject_GetAttrString,
    PyObject_GetIter, PyObject_Repr, PyRun_StringFlags, PySys_GetObject, PyThreadState,
    PyTuple_New, PyTuple_SetItem, PyUnicode_AsUTF8, PyUnicode_FromString, Py_DecRef, Py_IncRef,
    Py_XDECREF, Py_file_input, SubInterpreter, ThreadScope, PYTHON_MUTEX,
};
use crate::thread_bundle_map::{clear_current_thread_bundle, set_current_thread_bundle};
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::Arc;
use tracing::error;

// The exact Python script used in C++ for stdout/stderr redirection.
const STDOUT_REDIRECTION: &str = r"
import io, sys
class StdoutCatcher(io.TextIOBase):
    def write(self, msg):
        import _bundlelogging
        _bundlelogging.write(True, msg)


class StderrCatcher(io.TextIOBase):
    def write(self, msg):
        import _bundlelogging
        _bundlelogging.write(False, msg)


sys.stdout = StdoutCatcher()
sys.stderr = StderrCatcher()
";

struct BundleInterfaceInner {
    python_interpreter: SubInterpreter,
    p_global: *mut PyObject,
    p_bundle_module: *mut PyObject,
    json_module: *mut PyObject,
    traceback_module: *mut PyObject,
    bundle_hash: String,
}

unsafe impl Send for BundleInterfaceInner {}
unsafe impl Sync for BundleInterfaceInner {}

#[derive(Clone)]
pub struct BundleInterface {
    inner: Arc<BundleInterfaceInner>,
}

/// Custom error for when a Python function returns None
pub struct NoneException;

impl BundleInterface {
    /// Create a new `BundleInterface` for the given bundle hash.
    /// This exactly mirrors the C++ `BundleInterface` constructor.
    ///
    /// IMPORTANT: The `PYTHON_MUTEX` must NOT be held by the caller, and the
    /// main thread state must have been saved (GIL released) before calling this.
    #[allow(clippy::items_after_statements)]
    pub unsafe fn new(bundle_hash: &str, bundle_path_root: &str) -> Self {
        let _guard = PYTHON_MUTEX.lock();

        // Set up the thread bundle hash map (needed for logging during load)
        set_current_thread_bundle(bundle_hash.to_string());

        // C++ static local: save/restore the main thread state across
        // sub-interpreter creations.
        use std::sync::Mutex as StdMutex;
        struct SendPtr(*mut PyThreadState);
        unsafe impl Send for SendPtr {}
        static STATE: StdMutex<SendPtr> = StdMutex::new(SendPtr(std::ptr::null_mut()));
        {
            let mut state = STATE.lock().unwrap();
            if !state.0.is_null() {
                PyEval_RestoreThread(state.0);
                state.0 = std::ptr::null_mut();
            }
        }

        let python_interpreter = SubInterpreter::new();

        {
            let mut state = STATE.lock().unwrap();
            if state.0.is_null() {
                state.0 = PyEval_SaveThread();
            }
        }

        // Activate the new interpreter via ThreadScope
        let interp = python_interpreter.interp();
        let _scope = ThreadScope::new(interp);

        let bundle_path = Path::new(bundle_path_root).join(bundle_hash);

        // Create a new globals dict and enable the python builtins
        let p_global = PyDict_New();
        PyDict_SetItemString(p_global, c"__builtins__".as_ptr(), PyEval_GetBuiltins());

        // Set up logging so print() works as expected (run the redirection script)
        let p_local = PyDict_New();
        let c_redirect = CString::new(STDOUT_REDIRECTION).unwrap();
        let result = PyRun_StringFlags(
            c_redirect.as_ptr(),
            Py_file_input,
            p_global,
            p_local,
            std::ptr::null_mut(),
        );
        // Like C++: PyUnicode_AsUTF8(PyObject_Repr(PyRun_String(...)))
        if !result.is_null() {
            let repr = PyObject_Repr(result);
            if !repr.is_null() {
                PyUnicode_AsUTF8(repr);
                Py_DecRef(repr);
            }
            Py_DecRef(result);
        }
        Py_DecRef(p_local);

        // Ensure the json module is loaded in the global scope
        let json_module = PyImport_ImportModule(c"json".as_ptr());
        PyDict_SetItemString(p_global, c"json".as_ptr(), json_module);

        // Load the traceback module
        let traceback_module = PyImport_ImportModule(c"traceback".as_ptr());

        // Add the bundle path to the system path
        let p_path = PySys_GetObject(c"path".as_ptr());
        let c_bundle_path = CString::new(bundle_path.to_str().unwrap()).unwrap();
        let p_bundle_path = PyUnicode_FromString(c_bundle_path.as_ptr());
        PyList_Append(p_path, p_bundle_path);
        Py_DecRef(p_bundle_path);

        // Import the bundle module
        let p_bundle_module = PyImport_ImportModule(c"bundle".as_ptr());
        if !PyErr_Occurred().is_null() {
            error!("Error loading python bundle at path {:?}", bundle_path);
            PyErr_Print();
            panic!("Failed to load bundle module");
        }

        // Clear the thread from the thread bundle hash map
        clear_current_thread_bundle();

        BundleInterface {
            inner: Arc::new(BundleInterfaceInner {
                python_interpreter,
                p_global,
                p_bundle_module,
                json_module,
                traceback_module,
                bundle_hash: bundle_hash.to_string(),
            }),
        }
    }

    /// Get a `ThreadScope` for this bundle's interpreter.
    /// Equivalent to C++ `bundle->threadScope()`.
    pub unsafe fn thread_scope(&self) -> ThreadScope {
        ThreadScope::new(self.inner.python_interpreter.interp())
    }

    /// Run a bundle function. Mirrors C++ `BundleInterface::run()`.
    /// Returns the raw `PyObject`* result.
    pub unsafe fn run(
        &self,
        func: &str,
        details: &Value,
        job_data: &str,
    ) -> Result<*mut PyObject, NoneException> {
        // First create a python object from the details json
        let json_obj = self.json_loads(&serde_json::to_string(details).unwrap());

        // Get a pointer to the bundle function to call
        let s_func = CString::new(func).unwrap();
        let p_func = PyObject_GetAttrString(self.inner.p_bundle_module, s_func.as_ptr());

        // Check if function exists
        if p_func.is_null() || PyCallable_Check(p_func) == 0 {
            if !PyErr_Occurred().is_null() {
                // Clear the AttributeError
                let mut extype: *mut PyObject = std::ptr::null_mut();
                let mut value: *mut PyObject = std::ptr::null_mut();
                let mut tb: *mut PyObject = std::ptr::null_mut();
                PyErr_Fetch(&raw mut extype, &raw mut value, &raw mut tb);
                Py_XDECREF(extype);
                Py_XDECREF(value);
                Py_XDECREF(tb);
            }
            Py_XDECREF(p_func);
            Py_DecRef(json_obj);
            return Err(NoneException);
        }

        // Build a tuple to hold the arguments
        let p_args = PyTuple_New(2);
        PyTuple_SetItem(p_args, 0, json_obj);
        let p_job_data = PyUnicode_FromString(CString::new(job_data).unwrap().as_ptr());
        PyTuple_SetItem(p_args, 1, p_job_data);

        // Set up the thread bundle hash map
        set_current_thread_bundle(self.inner.bundle_hash.clone());

        // Call the bundle function
        let p_result = PyObject_CallObject(p_func, p_args);
        if !PyErr_Occurred().is_null() {
            error!("Error calling bundle function {}", func);
            self.print_last_python_exception();
            clear_current_thread_bundle();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err(NoneException);
        }

        // Clear the thread from the thread bundle hash map
        clear_current_thread_bundle();

        Py_DecRef(p_args);
        Py_XDECREF(p_func);

        if MyPy_IsNone(p_result) {
            return Err(NoneException);
        }

        Ok(p_result)
    }

    /// Convert a `PyObject` to a Rust String. Mirrors C++ `BundleInterface::toString()`.
    #[allow(clippy::unused_self)]
    pub unsafe fn to_string_py(&self, obj: *mut PyObject) -> String {
        if obj.is_null() {
            return String::new();
        }
        let c_str = PyUnicode_AsUTF8(obj);
        if c_str.is_null() {
            return String::new();
        }
        CStr::from_ptr(c_str).to_string_lossy().into_owned()
    }

    /// Convert a `PyObject` to u64. Mirrors C++ `BundleInterface::toUint64()`.
    #[allow(clippy::unused_self)]
    pub unsafe fn to_uint64(&self, obj: *mut PyObject) -> u64 {
        if obj.is_null() {
            return 0;
        }
        PyLong_AsUnsignedLongLong(obj)
    }

    /// Convert a `PyObject` to bool. Mirrors C++ `BundleInterface::toBool()`.
    #[allow(clippy::unused_self)]
    pub unsafe fn to_bool(&self, obj: *mut PyObject) -> bool {
        if obj.is_null() {
            return false;
        }
        obj == my_py_true_struct()
    }

    /// Call json.dumps on a `PyObject`. Mirrors C++ `BundleInterface::jsonDumps()`.
    pub unsafe fn json_dumps(&self, obj: *mut PyObject) -> String {
        if obj.is_null() {
            return "null".to_string();
        }

        let p_func = PyObject_GetAttrString(self.inner.json_module, c"dumps".as_ptr());

        let p_args = PyTuple_New(1);
        Py_IncRef(obj); // INCREF before SetItem (which steals a ref) – matches C++
        PyTuple_SetItem(p_args, 0, obj);

        let p_value = PyObject_CallObject(p_func, p_args);
        if !PyErr_Occurred().is_null() {
            self.print_last_python_exception();
            panic!("Error calling json.dumps");
        }

        let result = self.to_string_py(p_value);

        Py_DecRef(p_args);
        Py_XDECREF(p_func);
        // Note: p_value's refcount was stolen by json.dumps, but we need to
        // decref it since CallObject returns a new reference
        if !p_value.is_null() {
            Py_DecRef(p_value);
        }

        result
    }

    /// Call json.loads on a string. Mirrors C++ `BundleInterface::jsonLoads()`.
    pub unsafe fn json_loads(&self, content: &str) -> *mut PyObject {
        let p_func = PyObject_GetAttrString(self.inner.json_module, c"loads".as_ptr());

        let p_args = PyTuple_New(1);
        let p_value = PyUnicode_FromString(CString::new(content).unwrap().as_ptr());
        PyTuple_SetItem(p_args, 0, p_value);

        let result = PyObject_CallObject(p_func, p_args);
        if !PyErr_Occurred().is_null() {
            self.print_last_python_exception();
            panic!("Error calling json.loads");
        }

        Py_DecRef(p_args);
        Py_XDECREF(p_func);

        result
    }

    /// Print the last Python exception. Mirrors C++ `BundleInterface::printLastPythonException()`.
    pub unsafe fn print_last_python_exception(&self) {
        let mut extype: *mut PyObject = std::ptr::null_mut();
        let mut value: *mut PyObject = std::ptr::null_mut();
        let mut traceback: *mut PyObject = std::ptr::null_mut();

        PyErr_Fetch(&raw mut extype, &raw mut value, &raw mut traceback);
        if extype.is_null() {
            tracing::info!("No active python exception to print");
            return;
        }

        let p_func =
            PyObject_GetAttrString(self.inner.traceback_module, c"format_exception".as_ptr());

        let p_args = PyTuple_New(3);
        PyTuple_SetItem(p_args, 0, extype);
        PyTuple_SetItem(p_args, 1, value);
        PyTuple_SetItem(p_args, 2, traceback);

        let p_lines = PyObject_CallObject(p_func, p_args);
        if !PyErr_Occurred().is_null() {
            error!("Error printing active python exception");
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return;
        }

        // Iterate over the lines
        if !p_lines.is_null() {
            let iter = PyObject_GetIter(p_lines);
            if !iter.is_null() {
                loop {
                    let item = PyIter_Next(iter);
                    if item.is_null() {
                        break;
                    }
                    let c_str = PyUnicode_AsUTF8(item);
                    if !c_str.is_null() {
                        let s = CStr::from_ptr(c_str).to_string_lossy();
                        tracing::info!("{}", s);
                    }
                    Py_DecRef(item);
                }
                Py_DecRef(iter);
            }
            Py_DecRef(p_lines);
        }

        Py_DecRef(p_args);
        Py_XDECREF(p_func);
    }

    /// Dispose a `PyObject`. Mirrors C++ `BundleInterface::disposeObject()`.
    #[allow(clippy::unused_self)]
    pub unsafe fn dispose_object(&self, obj: *mut PyObject) {
        if !obj.is_null() {
            Py_DecRef(obj);
        }
    }
}

impl Drop for BundleInterfaceInner {
    fn drop(&mut self) {
        // SubInterpreter's Drop handles Py_EndInterpreter.
        // The PyObjects (p_global, p_bundle_module, etc.) are owned by the
        // sub-interpreter and will be cleaned up when it is destroyed.
        // We do NOT manually decref them here because the sub-interpreter
        // teardown handles that.
    }
}
