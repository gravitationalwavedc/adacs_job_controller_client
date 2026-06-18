//! Port of C++ `BundleInterface`.
//!
//! Each `BundleInterface` owns a `SubInterpreter` and caches the loaded bundle
//! module plus json/traceback helpers.  The C++ version never creates
//! temporary thread-states for every call – instead it creates a `ThreadScope`
//! that lives for the duration of the call.  We replicate that here.

use crate::python_interface::{
    get_main_ts, my_py_none_struct, my_py_true_struct, MyPy_IsNone, PyCallable_Check, PyDict_New,
    PyDict_SetItemString, PyErr_Clear, PyErr_Fetch, PyErr_Occurred, PyErr_Print,
    PyEval_GetBuiltins, PyEval_RestoreThread, PyEval_SaveThread, PyImport_ImportModule,
    PyIter_Next, PyList_Append, PyLong_AsUnsignedLongLong, PyObject, PyObject_CallObject,
    PyObject_GetAttrString, PyObject_GetIter, PyObject_Repr, PyRun_StringFlags, PySys_GetObject,
    PyThreadState, PyTuple_New, PyTuple_SetItem, PyUnicode_AsUTF8, PyUnicode_FromString, Py_DecRef,
    Py_IncRef, Py_XDECREF, Py_file_input, SubInterpreter, ThreadScope, PYTHON_MUTEX,
};
use crate::thread_bundle_map::{
    clear_current_thread_bundle, set_current_thread_bundle, ThreadBundleGuard,
};
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, error, info, trace};

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
        debug!("BundleInterface::new start for {}", bundle_hash);

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
            if state.0.is_null() {
                state.0 = get_main_ts();
            }
            if !state.0.is_null() {
                info!("BundleInterface::new restoring saved main thread state");
                PyEval_RestoreThread(state.0);
                state.0 = std::ptr::null_mut();
            }
        }

        debug!("BundleInterface::new creating sub-interpreter");
        let python_interpreter = SubInterpreter::new();
        debug!("BundleInterface::new created sub-interpreter");

        {
            let mut state = STATE.lock().unwrap();
            if state.0.is_null() {
                info!("BundleInterface::new saving main thread state");
                state.0 = PyEval_SaveThread();
            }
        }

        // Activate the new interpreter via ThreadScope
        let interp = python_interpreter.interp();
        debug!("BundleInterface::new creating thread scope");
        let _scope = ThreadScope::new(interp);
        debug!("BundleInterface::new created thread scope");

        let bundle_path = Path::new(bundle_path_root).join(bundle_hash);
        debug!("BundleInterface::new bundle path {:?}", bundle_path);

        // Create a new globals dict and enable the python builtins
        let p_global = PyDict_New();
        PyDict_SetItemString(p_global, c"__builtins__".as_ptr(), PyEval_GetBuiltins());

        // Set up logging so print() works as expected (run the redirection script)
        let p_local = PyDict_New();
        let c_redirect = CString::new(STDOUT_REDIRECTION).unwrap();
        debug!("BundleInterface::new installing stdout/stderr redirection");
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
        debug!("BundleInterface::new importing json");
        let json_module = PyImport_ImportModule(c"json".as_ptr());
        PyDict_SetItemString(p_global, c"json".as_ptr(), json_module);

        // Load the traceback module
        debug!("BundleInterface::new importing traceback");
        let traceback_module = PyImport_ImportModule(c"traceback".as_ptr());

        // Add the bundle path to the system path
        info!("BundleInterface::new appending bundle path to sys.path");
        let p_path = PySys_GetObject(c"path".as_ptr());
        let c_bundle_path = CString::new(bundle_path.to_string_lossy().as_ref())
            .expect("Bundle path contains NUL byte");
        let p_bundle_path = PyUnicode_FromString(c_bundle_path.as_ptr());
        PyList_Append(p_path, p_bundle_path);
        Py_DecRef(p_bundle_path);

        // Import the bundle module
        debug!("BundleInterface::new importing bundle module");
        let p_bundle_module = PyImport_ImportModule(c"bundle".as_ptr());
        if p_bundle_module.is_null() || !PyErr_Occurred().is_null() {
            error!("Error loading python bundle at path {:?}", bundle_path);
            PyErr_Print();
            clear_current_thread_bundle();
            panic!("Failed to load bundle module");
        }

        // Clear the thread from the thread bundle hash map
        debug!("BundleInterface::new finished for {}", bundle_hash);
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

        // Set up the thread bundle hash map (RAII guard clears on drop)
        let bundle_guard = ThreadBundleGuard::new(self.inner.bundle_hash.clone());

        // Call the bundle function
        debug!(
            "bundle: calling function {} for hash {} with details={}",
            func, self.inner.bundle_hash, details
        );
        let call_start = std::time::Instant::now();
        let p_result = PyObject_CallObject(p_func, p_args);
        let call_time = call_start.elapsed();
        if !PyErr_Occurred().is_null() {
            error!(
                "Error calling bundle function {} after {:?} for hash {}",
                func, call_time, self.inner.bundle_hash
            );
            self.print_last_python_exception();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err(NoneException);
        }
        debug!(
            "bundle: function {} returned successfully after {:?} for hash {}",
            func, call_time, self.inner.bundle_hash
        );

        drop(bundle_guard);
        Py_DecRef(p_args);
        Py_XDECREF(p_func);

        if MyPy_IsNone(p_result) {
            Py_DecRef(p_result);
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
    ///
    /// Robust against two failure modes that the original single-call
    /// `traceback.format_exception(etype, value, tb)` could not handle:
    ///
    /// 1. `value` is a raw `str` (not an exception instance). This happens
    ///    when a `_bundledb.error` raised via `PyErr_SetString` falls through
    ///    under Python 3.12's per-interpreter GIL (PEP 684) — the type
    ///    object from one sub-interpreter may not be valid in another, so
    ///    `error_obj(msg)` fails silently and the raw message is stored as
    ///    `value` instead. On modern Python, both `format_exception` and
    ///    `format_exception_only` then fail because they expect an exception
    ///    instance and touch attributes like `__suppress_context__` that a
    ///    raw `str` does not have.
    /// 2. `traceback` is NULL. `format_exception` requires a non-NULL
    ///    `traceback`; `format_tb` returns `[]` for None/NULL.
    ///
    /// We split the work into two independent calls — `format_tb(tb)` and
    /// `format_exception_only(etype, value)`. The traceback-frame call is
    /// the important part for preserving debugging value: even if formatting
    /// the final exception line still fails, we keep the frame list and then
    /// synthesize a final `type: value` line from the already-extracted Rust
    /// strings. We also defensively skip `format_tb` entirely when `tb` is
    /// NULL, and pass `my_py_none_struct()` to `format_exception_only` when
    /// `value` is NULL.
    pub unsafe fn print_last_python_exception(&self) {
        let mut extype: *mut PyObject = std::ptr::null_mut();
        let mut value: *mut PyObject = std::ptr::null_mut();
        let mut traceback: *mut PyObject = std::ptr::null_mut();

        PyErr_Fetch(&raw mut extype, &raw mut value, &raw mut traceback);
        if extype.is_null() {
            trace!("No active python exception to print");
            return;
        }

        // Always log the raw type and value first so that even if every
        // traceback formatting call below fails we still have a record.
        let type_name = extract_type_name(extype);
        let value_str = extract_value_repr(value);
        let value_display = extract_value_display(value);
        error!(
            "Python exception: type={} value=\"{}\"",
            type_name, value_str
        );

        // Step 2: format the traceback frames, if any. NULL `traceback` is
        // valid (means "no frames"); we just skip the call entirely.
        if !traceback.is_null() {
            let tb_func =
                PyObject_GetAttrString(self.inner.traceback_module, c"format_tb".as_ptr());
            if tb_func.is_null() {
                swallow_python_error();
            } else {
                let tb_args = PyTuple_New(1);
                PyTuple_SetItem(tb_args, 0, traceback); // steals the ref
                let tb_lines = PyObject_CallObject(tb_func, tb_args);
                let tb_ok = !tb_lines.is_null() && PyErr_Occurred().is_null();
                if tb_ok {
                    error!("Traceback (most recent call last):");
                    log_python_lines(tb_lines);
                    Py_DecRef(tb_lines);
                } else {
                    error!(
                        "Error formatting python traceback frames (type was {})",
                        type_name
                    );
                    if !tb_lines.is_null() {
                        Py_DecRef(tb_lines);
                    }
                    swallow_python_error();
                }
                // tb_args stole the `traceback` ref; releasing the tuple
                // decrefs traceback too. Safe in both success and failure.
                Py_DecRef(tb_args);
                Py_XDECREF(tb_func);
            }
        }

        // Step 3: format the exception header (type + value).
        let eo_func = PyObject_GetAttrString(
            self.inner.traceback_module,
            c"format_exception_only".as_ptr(),
        );
        if eo_func.is_null() {
            swallow_python_error();
            // Final fallback so the user is never left with no info.
            error!(
                "{}: {}",
                type_name,
                fallback_value_text(&value_display, &value_str)
            );
        } else {
            let eo_args = PyTuple_New(2);
            PyTuple_SetItem(eo_args, 0, extype); // steals the ref
                                                 // `format_exception_only` still expects an exception-like object
                                                 // on modern Python, so raw-string values may make it fail. We
                                                 // keep a manual fallback below. Pass `Py_None` only when the
                                                 // fetched value is literally NULL.
            if value.is_null() {
                let none = my_py_none_struct();
                Py_IncRef(none);
                PyTuple_SetItem(eo_args, 1, none);
            } else {
                PyTuple_SetItem(eo_args, 1, value); // steals the ref
            }
            let eo_lines = PyObject_CallObject(eo_func, eo_args);
            let eo_ok = !eo_lines.is_null() && PyErr_Occurred().is_null();
            if eo_ok {
                log_python_lines(eo_lines);
                Py_DecRef(eo_lines);
            } else {
                // Final fallback so the user is never left with no info.
                error!(
                    "{}: {}",
                    type_name,
                    fallback_value_text(&value_display, &value_str)
                );
                debug!(
                    "Falling back to synthesized python exception header (type was {})",
                    type_name
                );
                if !eo_lines.is_null() {
                    Py_DecRef(eo_lines);
                }
                swallow_python_error();
            }
            // eo_args stole both extype and value refs; releasing the tuple
            // decrefs both. Safe in both success and failure paths.
            Py_DecRef(eo_args);
            Py_XDECREF(eo_func);
        }
    }

    /// Dispose a `PyObject`. Mirrors C++ `BundleInterface::disposeObject()`.
    #[allow(clippy::unused_self)]
    pub unsafe fn dispose_object(&self, obj: *mut PyObject) {
        if !obj.is_null() {
            Py_DecRef(obj);
        }
    }
}

// ─── Free-standing helpers (used by `print_last_python_exception`) ───────────

/// Extract the `__name__` attribute of a Python type as a Rust `String`.
/// Returns `"unknown"` if the attribute lookup itself fails. Any Python
/// error raised by the lookup is fetched and discarded so the caller's
/// error state is not corrupted.
unsafe fn extract_type_name(extype: *mut PyObject) -> String {
    let type_str = PyObject_GetAttrString(extype, c"__name__".as_ptr());
    if type_str.is_null() {
        swallow_python_error();
        return "unknown".to_string();
    }
    let c_str = PyUnicode_AsUTF8(type_str);
    let name = if c_str.is_null() {
        "unknown".to_string()
    } else {
        CStr::from_ptr(c_str).to_string_lossy().into_owned()
    };
    Py_DecRef(type_str);
    name
}

/// Extract the `repr()` of a Python object as a Rust `String`. Returns
/// `""` if `value` is NULL or `repr()` fails. Any Python error raised by
/// the conversion is fetched and discarded.
unsafe fn extract_value_repr(value: *mut PyObject) -> String {
    if value.is_null() {
        return String::new();
    }
    let str_obj = PyObject_Repr(value);
    if str_obj.is_null() {
        swallow_python_error();
        return String::new();
    }
    let c_str = PyUnicode_AsUTF8(str_obj);
    let s = if c_str.is_null() {
        String::new()
    } else {
        CStr::from_ptr(c_str).to_string_lossy().into_owned()
    };
    Py_DecRef(str_obj);
    s
}

/// Extract the `str()` of a Python object as a Rust `String`. Returns `""`
/// if `value` is NULL or `str()` fails. Any Python error raised by the
/// conversion is fetched and discarded.
unsafe fn extract_value_display(value: *mut PyObject) -> String {
    if value.is_null() {
        return String::new();
    }
    let str_obj = crate::python_interface::PyObject_Str(value);
    if str_obj.is_null() {
        swallow_python_error();
        return String::new();
    }
    let c_str = PyUnicode_AsUTF8(str_obj);
    let s = if c_str.is_null() {
        String::new()
    } else {
        CStr::from_ptr(c_str).to_string_lossy().into_owned()
    };
    Py_DecRef(str_obj);
    s
}

fn fallback_value_text<'a>(display: &'a str, repr: &'a str) -> &'a str {
    if display.is_empty() {
        repr
    } else {
        display
    }
}

/// Iterate a Python iterable of strings and log each line via
/// `tracing::info!`. Returns silently on NULL or non-iterable input.
unsafe fn log_python_lines(lines: *mut PyObject) {
    if lines.is_null() {
        return;
    }
    let iter = PyObject_GetIter(lines);
    if iter.is_null() {
        return;
    }
    loop {
        let item = PyIter_Next(iter);
        if item.is_null() {
            break;
        }
        let c_str = PyUnicode_AsUTF8(item);
        if !c_str.is_null() {
            let s = CStr::from_ptr(c_str).to_string_lossy();
            error!("{}", s);
        }
        Py_DecRef(item);
    }
    Py_DecRef(iter);
}

/// Fetch and discard any active Python error, also clearing the
/// thread's error indicator. Used by `print_last_python_exception`
/// to recover from a failed fallback call without poisoning the next
/// Python C-API call on this thread.
unsafe fn swallow_python_error() {
    let mut ex: *mut PyObject = std::ptr::null_mut();
    let mut val: *mut PyObject = std::ptr::null_mut();
    let mut tb: *mut PyObject = std::ptr::null_mut();
    PyErr_Fetch(&raw mut ex, &raw mut val, &raw mut tb);
    Py_XDECREF(ex);
    Py_XDECREF(val);
    Py_XDECREF(tb);
    // Defensive: clear the indicator in case PyErr_Fetch did not (e.g.
    // a sub-interpreter-local error state was already cleared).
    PyErr_Clear();
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
