//! Port of C++ `BundleInterface`.
//!
//! Each `BundleInterface` owns a `SubInterpreter` and caches the loaded bundle
//! module plus json/traceback helpers.  The C++ version never creates
//! temporary thread-states for every call – instead it creates a `ThreadScope`
//! that lives for the duration of the call.  We replicate that here.

use crate::python_interface::{
    get_main_ts, my_py_none_struct, my_py_true_struct, py_tuple_set_item, MyPy_IsNone,
    PyCallable_Check, PyDict_New, PyDict_SetItemString, PyErr_Clear, PyErr_Fetch, PyErr_Occurred,
    PyErr_Print, PyEval_GetBuiltins, PyEval_RestoreThread, PyEval_SaveThread,
    PyImport_ImportModule, PyIter_Next, PyList_Append, PyLong_AsUnsignedLongLong, PyObject,
    PyObject_CallObject, PyObject_GetAttrString, PyObject_GetIter, PyObject_Repr,
    PyRun_StringFlags, PySys_GetObject, PyThreadState, PyTuple_New, PyTuple_SetItem,
    PyUnicode_AsUTF8, PyUnicode_FromString, Py_DecRef, Py_IncRef, Py_XDECREF, Py_file_input,
    SubInterpreter, ThreadScope, PYTHON_MUTEX,
};
use crate::thread_bundle_map::ThreadBundleGuard;
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
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
    #[cfg(test)]
    thread_scope_error: Option<String>,
}

// SAFETY: All raw pointer fields (p_global, p_bundle_module, json_module,
// traceback_module) are owned by the sub-interpreter and only accessed while
// holding PYTHON_MUTEX. SubInterpreter is Send + Sync; String is Send + Sync.
unsafe impl Send for BundleInterfaceInner {}
// SAFETY: Same invariants as Send — raw pointer fields are owned by the
// sub-interpreter and only accessed while holding PYTHON_MUTEX.
unsafe impl Sync for BundleInterfaceInner {}

// SAFETY: The wrapped `*mut PyThreadState` is only accessed while holding
// PYTHON_MUTEX (all accesses to STATE happen inside BundleInterface::new,
// which holds the mutex), so moving it between threads is safe.
struct SendPtr(*mut PyThreadState);
unsafe impl Send for SendPtr {}
// C++ static local: save/restore the main thread state across
// sub-interpreter creations.
static STATE: StdMutex<SendPtr> = StdMutex::new(SendPtr(std::ptr::null_mut()));

#[derive(Clone)]
pub struct BundleInterface {
    inner: Arc<BundleInterfaceInner>,
}

// ─── Test-only json_loads override seam ──────────────────────────────────────
// The `json_obj.is_null()` early-return branch in `BundleInterface::run` is
// unreachable through the public API because `run` always serializes valid
// JSON, so `json_loads` always returns a non-NULL object. This seam lets tests
// force `json_loads` to return NULL without changing production behavior.
// Tests run serially (`--test-threads=1`), so the global override cannot race
// across tests.

#[cfg(test)]
pub type JsonLoadsFn = unsafe fn(&BundleInterface, &str) -> *mut PyObject;

#[cfg(test)]
static JSON_LOADS_OVERRIDE: StdMutex<Option<JsonLoadsFn>> = StdMutex::new(None);

/// Test-only: install an override for `BundleInterface::json_loads`, returning
/// the previously-installed override (if any). Pass `None` to clear it.
#[cfg(test)]
pub fn set_json_loads_override(f: Option<JsonLoadsFn>) -> Option<JsonLoadsFn> {
    let mut guard = JSON_LOADS_OVERRIDE
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    std::mem::replace(&mut *guard, f)
}

impl BundleInterface {
    /// Return the bundle hash for this interface.
    pub fn bundle_hash(&self) -> &str {
        &self.inner.bundle_hash
    }

    /// Test-only: create a `BundleInterface` whose `thread_scope()` always
    /// fails with `error`. Used to exercise the thread-scope error branches in
    /// `BundleManager` without depending on `PyThreadState_New` allocation
    /// failure (which is not reliably triggerable).
    #[cfg(test)]
    pub fn with_thread_scope_error(bundle_hash: &str, error: &str) -> Self {
        BundleInterface {
            inner: Arc::new(BundleInterfaceInner {
                python_interpreter: SubInterpreter::null(),
                p_global: std::ptr::null_mut(),
                p_bundle_module: std::ptr::null_mut(),
                json_module: std::ptr::null_mut(),
                traceback_module: std::ptr::null_mut(),
                bundle_hash: bundle_hash.to_string(),
                thread_scope_error: Some(error.to_string()),
            }),
        }
    }
}

/// Custom error for when a Python function returns None
pub struct NoneException;

impl BundleInterface {
    /// Create a new `BundleInterface` for the given bundle hash.
    /// This exactly mirrors the C++ `BundleInterface` constructor.
    ///
    /// IMPORTANT: The `PYTHON_MUTEX` must NOT be held by the caller, and the
    /// main thread state must have been saved (GIL released) before calling this.
    pub unsafe fn new(bundle_hash: &str, bundle_path_root: &str) -> Result<Self, String> {
        let _guard = PYTHON_MUTEX.lock();
        debug!("BundleInterface::new start for {}", bundle_hash);

        // Set up the thread bundle hash map (needed for logging during load)
        let _bundle_guard = ThreadBundleGuard::new(bundle_hash.to_string());

        {
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
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
        let python_interpreter = match SubInterpreter::new() {
            Ok(interp) => interp,
            Err(e) => {
                // Mirror the success path below: the main thread state was
                // restored (GIL acquired) above, so re-save it before
                // propagating the error, or the GIL is leaked and all
                // subsequent Python FFI calls deadlock.
                let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
                if state.0.is_null() {
                    info!("BundleInterface::new saving main thread state");
                    state.0 = PyEval_SaveThread();
                }
                return Err(e);
            }
        };
        debug!("BundleInterface::new created sub-interpreter");

        {
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            if state.0.is_null() {
                info!("BundleInterface::new saving main thread state");
                state.0 = PyEval_SaveThread();
            }
        }

        // Activate the new interpreter via ThreadScope
        let interp = python_interpreter.interp();
        debug!("BundleInterface::new creating thread scope");
        let _scope = ThreadScope::new(interp)?;
        debug!("BundleInterface::new created thread scope");

        let bundle_path = Path::new(bundle_path_root).join(bundle_hash);
        debug!("BundleInterface::new bundle path {:?}", bundle_path);

        // Create a new globals dict and enable the python builtins
        let p_global = PyDict_New();
        if p_global.is_null() {
            error!("Error creating global dict");
            PyErr_Print();
            return Err("Failed to create global dict".to_string());
        }
        if PyDict_SetItemString(p_global, c"__builtins__".as_ptr(), PyEval_GetBuiltins()) < 0 {
            error!("Error setting __builtins__ in globals dict");
            PyErr_Print();
            Py_DecRef(p_global);
            return Err("Failed to set __builtins__ in globals dict".to_string());
        }

        // Set up logging so print() works as expected (run the redirection script)
        let p_local = PyDict_New();
        if p_local.is_null() {
            error!("Error creating local dict");
            PyErr_Print();
            Py_DecRef(p_global);
            return Err("Failed to create local dict".to_string());
        }
        let c_redirect = CString::new(STDOUT_REDIRECTION).unwrap();
        debug!("BundleInterface::new installing stdout/stderr redirection");
        let result = PyRun_StringFlags(
            c_redirect.as_ptr(),
            Py_file_input,
            p_global,
            p_local,
            std::ptr::null_mut(),
        );
        if result.is_null() {
            error!("Error installing stdout/stderr redirection");
            PyErr_Print();
            Py_DecRef(p_global);
            Py_DecRef(p_local);
            return Err("Failed to install stdout/stderr redirection".to_string());
        }

        Py_DecRef(result);
        Py_DecRef(p_local);

        // Ensure the json module is loaded in the global scope
        debug!("BundleInterface::new importing json");
        let json_module = PyImport_ImportModule(c"json".as_ptr());
        if json_module.is_null() {
            error!("Error importing json module");
            PyErr_Print();
            Py_DecRef(p_global);
            return Err("Failed to import json module".to_string());
        }
        if PyDict_SetItemString(p_global, c"json".as_ptr(), json_module) < 0 {
            error!("Error setting json module in globals dict");
            PyErr_Print();
            Py_DecRef(json_module);
            Py_DecRef(p_global);
            return Err("Failed to set json module in globals dict".to_string());
        }

        // Load the traceback module
        debug!("BundleInterface::new importing traceback");
        let traceback_module = PyImport_ImportModule(c"traceback".as_ptr());
        if traceback_module.is_null() {
            error!("Error importing traceback module");
            PyErr_Print();
            Py_DecRef(json_module);
            Py_DecRef(p_global);
            return Err("Failed to import traceback module".to_string());
        }

        // Add the bundle path to the system path
        info!("BundleInterface::new appending bundle path to sys.path");
        let p_path = PySys_GetObject(c"path".as_ptr());
        if let Err(e) = Self::append_bundle_path_to_sys_path(p_path, &bundle_path) {
            Py_DecRef(p_global);
            Py_DecRef(json_module);
            Py_DecRef(traceback_module);
            return Err(e);
        }

        // Import the bundle module
        debug!("BundleInterface::new importing bundle module");
        let p_bundle_module = PyImport_ImportModule(c"bundle".as_ptr());
        if p_bundle_module.is_null() || !PyErr_Occurred().is_null() {
            error!("Error loading python bundle at path {:?}", bundle_path);
            PyErr_Print();
            // Release the globals dict and the json/traceback module references
            // so a failed bundle load does not leak them.
            Py_DecRef(p_global);
            Py_DecRef(json_module);
            Py_DecRef(traceback_module);
            Py_XDECREF(p_bundle_module);
            return Err("Failed to load bundle module".to_string());
        }

        // Clear the thread from the thread bundle hash map
        debug!("BundleInterface::new finished for {}", bundle_hash);

        Ok(BundleInterface {
            inner: Arc::new(BundleInterfaceInner {
                python_interpreter,
                p_global,
                p_bundle_module,
                json_module,
                traceback_module,
                bundle_hash: bundle_hash.to_string(),
                #[cfg(test)]
                thread_scope_error: None,
            }),
        })
    }

    /// Append `bundle_path` to the `sys.path` list of the current interpreter.
    ///
    /// Returns `Err` when `p_path` is NULL (a missing `sys.path`) or when
    /// `bundle_path` contains a NUL byte.
    ///
    /// # Safety
    /// `p_path` must be NULL or a valid `sys.path` list on the current
    /// interpreter, and the GIL must be held by the caller.
    unsafe fn append_bundle_path_to_sys_path(
        p_path: *mut PyObject,
        bundle_path: &Path,
    ) -> Result<(), String> {
        if p_path.is_null() {
            error!("Error getting sys.path");
            PyErr_Print();
            return Err("Failed to get sys.path".to_string());
        }
        let c_bundle_path = CString::new(bundle_path.to_string_lossy().as_ref())
            .map_err(|_| "Bundle path contains NUL byte".to_string())?;
        let p_bundle_path = PyUnicode_FromString(c_bundle_path.as_ptr());
        if p_bundle_path.is_null() {
            error!("Error creating bundle path string");
            PyErr_Print();
            return Err("Failed to create bundle path string".to_string());
        }
        if PyList_Append(p_path, p_bundle_path) == -1 {
            error!("Error appending bundle path to sys.path");
            PyErr_Print();
            Py_DecRef(p_bundle_path);
            return Err("Failed to append bundle path to sys.path".to_string());
        }
        Py_DecRef(p_bundle_path);
        Ok(())
    }

    /// Get a `ThreadScope` for this bundle's interpreter.
    /// Equivalent to C++ `bundle->threadScope()`.
    pub unsafe fn thread_scope(&self) -> Result<ThreadScope, String> {
        #[cfg(test)]
        if let Some(e) = &self.inner.thread_scope_error {
            return Err(e.clone());
        }
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
        let Ok(json_str) = serde_json::to_string(details) else {
            return Err(NoneException);
        };
        let json_obj = self.json_loads(&json_str);
        if json_obj.is_null() {
            return Err(NoneException);
        }

        // Get a pointer to the bundle function to call
        let Ok(s_func) = CString::new(func) else {
            Py_DecRef(json_obj);
            return Err(NoneException);
        };
        let p_func = PyObject_GetAttrString(self.inner.p_bundle_module, s_func.as_ptr());

        // Check if function exists
        if p_func.is_null() {
            // PyObject_GetAttrString set an AttributeError; clear it
            swallow_python_error();
            Py_XDECREF(p_func);
            Py_DecRef(json_obj);
            return Err(NoneException);
        }
        if PyCallable_Check(p_func) == 0 {
            // Attribute exists but is not callable; PyCallable_Check returns 0
            // without setting a Python error, so no error needs clearing.
            Py_XDECREF(p_func);
            Py_DecRef(json_obj);
            return Err(NoneException);
        }

        // Build a tuple to hold the arguments
        let p_args = PyTuple_New(2);
        if p_args.is_null() {
            Py_XDECREF(p_func);
            Py_DecRef(json_obj);
            return Err(NoneException);
        }
        // On failure PyTuple_SetItem releases the item reference itself, so we
        // must not Py_DecRef the item again here.
        if PyTuple_SetItem(p_args, 0, json_obj) < 0 {
            error!("Error setting json object in args tuple");
            PyErr_Print();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err(NoneException);
        }
        let Ok(c_job_data) = CString::new(job_data) else {
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err(NoneException);
        };
        let p_job_data = PyUnicode_FromString(c_job_data.as_ptr());
        if p_job_data.is_null() {
            swallow_python_error();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err(NoneException);
        }
        // On failure PyTuple_SetItem releases the item reference itself, so we
        // must not Py_DecRef the item again here.
        if PyTuple_SetItem(p_args, 1, p_job_data) < 0 {
            error!("Error setting job data in args tuple");
            PyErr_Print();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err(NoneException);
        }

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
        if p_result.is_null() || !PyErr_Occurred().is_null() {
            error!(
                "Error calling bundle function {} after {:?} for hash {}",
                func, call_time, self.inner.bundle_hash
            );
            self.print_last_python_exception();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            Py_XDECREF(p_result);
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
    pub unsafe fn to_string_py(obj: *mut PyObject) -> String {
        if obj.is_null() {
            return String::new();
        }
        let c_str = PyUnicode_AsUTF8(obj);
        if c_str.is_null() {
            // PyUnicode_AsUTF8 sets a TypeError when `obj` is not a str;
            // clear it so the stale error can't poison later FFI calls.
            PyErr_Clear();
            return String::new();
        }
        CStr::from_ptr(c_str).to_string_lossy().into_owned()
    }

    /// Convert a `PyObject` to u64. Mirrors C++ `BundleInterface::toUint64()`.
    pub unsafe fn to_uint64(obj: *mut PyObject) -> u64 {
        if obj.is_null() {
            return 0;
        }
        let result = PyLong_AsUnsignedLongLong(obj);
        if !PyErr_Occurred().is_null() {
            PyErr_Clear();
            return 0;
        }
        result
    }

    /// Convert a `PyObject` to bool. Mirrors C++ `BundleInterface::toBool()`.
    pub unsafe fn to_bool(obj: *mut PyObject) -> bool {
        if obj.is_null() {
            return false;
        }
        obj == my_py_true_struct()
    }

    /// Call json.dumps on a `PyObject`. Mirrors C++ `BundleInterface::jsonDumps()`.
    pub unsafe fn json_dumps(&self, obj: *mut PyObject) -> Result<String, String> {
        if obj.is_null() {
            return Ok("null".to_string());
        }

        let p_func = PyObject_GetAttrString(self.inner.json_module, c"dumps".as_ptr());
        if p_func.is_null() {
            PyErr_Clear();
            return Err("Failed to get json.dumps function".to_string());
        }

        let p_args = PyTuple_New(1);
        if p_args.is_null() {
            Py_XDECREF(p_func);
            PyErr_Clear();
            return Err("Failed to allocate argument tuple".to_string());
        }
        // INCREF before SetItem (which steals a ref) – matches C++
        Py_IncRef(obj);
        // On failure PyTuple_SetItem releases the item reference itself, so we
        // must not Py_DecRef the item again here.
        if PyTuple_SetItem(p_args, 0, obj) < 0 {
            error!("Error setting object in args tuple");
            PyErr_Print();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return Err("Error calling json.dumps".to_string());
        }

        let p_value = PyObject_CallObject(p_func, p_args);
        if p_value.is_null() || !PyErr_Occurred().is_null() {
            self.print_last_python_exception();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            Py_XDECREF(p_value);
            return Err("Error calling json.dumps".to_string());
        }

        let result = Self::to_string_py(p_value);

        Py_DecRef(p_args);
        Py_XDECREF(p_func);
        // Note: CallObject returns a new reference owned by the caller, so
        // we need to decref p_value
        Py_DecRef(p_value);

        Ok(result)
    }

    /// Call json.loads on a string. Mirrors C++ `BundleInterface::jsonLoads()`.
    pub unsafe fn json_loads(&self, content: &str) -> *mut PyObject {
        #[cfg(test)]
        if let Some(f) = *JSON_LOADS_OVERRIDE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            return f(self, content);
        }

        let p_func = PyObject_GetAttrString(self.inner.json_module, c"loads".as_ptr());
        if p_func.is_null() {
            error!("json_loads: failed to get json.loads function");
            PyErr_Clear();
            return std::ptr::null_mut();
        }

        let p_args = PyTuple_New(1);
        if p_args.is_null() {
            error!("json_loads: failed to create arguments tuple");
            Py_XDECREF(p_func);
            PyErr_Clear();
            return std::ptr::null_mut();
        }
        let c_content = match CString::new(content) {
            Ok(c) => c,
            Err(e) => {
                error!("json_loads: content contains NUL byte: {}", e);
                Py_DecRef(p_args);
                Py_XDECREF(p_func);
                return std::ptr::null_mut();
            }
        };
        let p_value = PyUnicode_FromString(c_content.as_ptr());
        if p_value.is_null() {
            error!("json_loads: failed to create python string");
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            PyErr_Clear();
            return std::ptr::null_mut();
        }
        // On failure PyTuple_SetItem releases the item reference itself, so we
        // must not Py_DecRef the item again here.
        if PyTuple_SetItem(p_args, 0, p_value) < 0 {
            error!("Error setting object in args tuple");
            PyErr_Print();
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            return std::ptr::null_mut();
        }

        let result = PyObject_CallObject(p_func, p_args);
        if result.is_null() || !PyErr_Occurred().is_null() {
            self.print_last_python_exception();
            error!("Error calling json.loads - returning NULL");
            Py_DecRef(p_args);
            Py_XDECREF(p_func);
            Py_XDECREF(result);
            return std::ptr::null_mut();
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
        let value_str = extract_value_string(value, PyObject_Repr);
        let value_display = extract_value_string(value, crate::python_interface::PyObject_Str);
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
                // `tb_args` was never created, so `traceback` was never stolen
                // into a tuple; release the owned ref from `PyErr_Fetch`.
                Py_XDECREF(traceback);
                swallow_python_error();
            } else {
                let tb_args = PyTuple_New(1);
                if tb_args.is_null() {
                    // The `traceback` ref is still owned here; release it and
                    // the function ref before falling through to the header.
                    Py_XDECREF(traceback);
                    Py_XDECREF(tb_func);
                    swallow_python_error();
                    error!(
                        "Error formatting python traceback frames (type was {})",
                        type_name
                    );
                } else {
                    // On failure PyTuple_SetItem releases the item reference itself, so we
                    // must not Py_DecRef the item again here.
                    if py_tuple_set_item(tb_args, 0, traceback) < 0 {
                        error!("Error setting traceback in args tuple");
                        PyErr_Print();
                        Py_DecRef(tb_args);
                        Py_XDECREF(tb_func);
                    } else {
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
            }
        }

        // Step 3: format the exception header (type + value).
        let eo_func = PyObject_GetAttrString(
            self.inner.traceback_module,
            c"format_exception_only".as_ptr(),
        );
        if eo_func.is_null() {
            // `eo_args` was never created, so `extype` and `value` were never
            // stolen into a tuple; release the owned refs from `PyErr_Fetch`.
            Py_XDECREF(extype);
            Py_XDECREF(value);
            swallow_python_error();
            // Final fallback so the user is never left with no info.
            error!(
                "{}: {}",
                type_name,
                fallback_value_text(&value_display, &value_str)
            );
        } else {
            let eo_args = PyTuple_New(2);
            if eo_args.is_null() {
                // The `extype` and `value` refs are still owned here; release
                // them and the function ref before the fallback.
                Py_XDECREF(extype);
                Py_XDECREF(value);
                Py_XDECREF(eo_func);
                swallow_python_error();
                // Final fallback so the user is never left with no info.
                error!(
                    "{}: {}",
                    type_name,
                    fallback_value_text(&value_display, &value_str)
                );
            } else {
                // On failure PyTuple_SetItem releases the item reference itself, so we
                // must not Py_DecRef the item again here.
                if py_tuple_set_item(eo_args, 0, extype) < 0 {
                    error!("Error setting exception type in args tuple");
                    PyErr_Print();
                    Py_DecRef(eo_args);
                    // `value` has not been consumed by any SetItem yet; release it
                    // so it is not leaked.
                    Py_XDECREF(value);
                    Py_XDECREF(eo_func);
                    return;
                }
                // `format_exception_only` still expects an exception-like object
                // on modern Python, so raw-string values may make it fail. We
                // keep a manual fallback below. Pass `Py_None` only when the
                // fetched value is literally NULL.
                if !set_exception_value_slot(eo_args, value, eo_func) {
                    return;
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
    }

    /// Dispose a `PyObject`. Mirrors C++ `BundleInterface::disposeObject()`.
    pub unsafe fn dispose_object(obj: *mut PyObject) {
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
// SAFETY: Caller holds PYTHON_MUTEX and the bundle sub-interpreter GIL;
// `extype` is a live type object from `PyErr_Fetch` on this thread.
unsafe fn extract_type_name(extype: *mut PyObject) -> String {
    let type_str = PyObject_GetAttrString(extype, c"__name__".as_ptr());
    if type_str.is_null() {
        swallow_python_error();
        return "unknown".to_string();
    }
    let c_str = PyUnicode_AsUTF8(type_str);
    let name = if c_str.is_null() {
        PyErr_Clear();
        "unknown".to_string()
    } else {
        CStr::from_ptr(c_str).to_string_lossy().into_owned()
    };
    Py_DecRef(type_str);
    name
}

/// Convert a Python object to a Rust `String` using the given converter
/// (`PyObject_Repr` for `repr()`, `PyObject_Str` for `str()`). Returns `""`
/// if `value` is NULL or the conversion fails. Any Python error raised by
/// the conversion is fetched and discarded.
// SAFETY: Caller holds PYTHON_MUTEX and the bundle sub-interpreter GIL;
// `value` is NULL or a live object from `PyErr_Fetch` on this thread.
unsafe fn extract_value_string(
    value: *mut PyObject,
    converter: unsafe fn(*mut PyObject) -> *mut PyObject,
) -> String {
    if value.is_null() {
        return String::new();
    }
    let str_obj = converter(value);
    if str_obj.is_null() {
        swallow_python_error();
        return String::new();
    }
    let c_str = PyUnicode_AsUTF8(str_obj);
    let s = if c_str.is_null() {
        PyErr_Clear();
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

/// Set the exception-value slot (index 1) of `eo_args`, passing `Py_None`
/// when `value` is NULL. Returns `true` on success. On failure it logs the
/// marker, releases `eo_args` and `eo_func`, and returns `false` (the caller
/// must return early).
///
/// On failure `PyTuple_SetItem` releases the item reference itself, so we must
/// not `Py_DecRef` the item again here.
// SAFETY: Caller holds PYTHON_MUTEX and the bundle sub-interpreter GIL;
// `eo_args` is a live size-2 tuple, `value` is NULL or a live object, and
// `eo_func` is a live callable.
unsafe fn set_exception_value_slot(
    eo_args: *mut PyObject,
    value: *mut PyObject,
    eo_func: *mut PyObject,
) -> bool {
    if value.is_null() {
        let none = my_py_none_struct();
        Py_IncRef(none);
        if py_tuple_set_item(eo_args, 1, none) < 0 {
            error!("Error setting exception value in args tuple");
            PyErr_Print();
            Py_DecRef(eo_args);
            Py_XDECREF(eo_func);
            return false;
        }
    } else if py_tuple_set_item(eo_args, 1, value) < 0 {
        error!("Error setting exception value in args tuple");
        PyErr_Print();
        Py_DecRef(eo_args);
        Py_XDECREF(eo_func);
        return false;
    }
    true
}

/// Iterate a Python iterable of strings and log each line via
/// `tracing::info!`. Returns silently on NULL or non-iterable input.
// SAFETY: Caller holds PYTHON_MUTEX and the bundle sub-interpreter GIL;
// `lines` is NULL or a live list/iterable returned by traceback formatting.
unsafe fn log_python_lines(lines: *mut PyObject) {
    if lines.is_null() {
        return;
    }
    let iter = PyObject_GetIter(lines);
    if iter.is_null() {
        swallow_python_error();
        return;
    }
    loop {
        let item = PyIter_Next(iter);
        if item.is_null() {
            if !PyErr_Occurred().is_null() {
                swallow_python_error();
            }
            break;
        }
        let c_str = PyUnicode_AsUTF8(item);
        if c_str.is_null() {
            swallow_python_error();
        } else {
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
// SAFETY: Caller holds PYTHON_MUTEX and the bundle sub-interpreter GIL;
// only reads and clears this thread's Python error indicator.
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

#[cfg(test)]
mod fallback_value_text_tests {
    use super::fallback_value_text;

    #[test]
    fn prefers_display_when_nonempty() {
        assert_eq!(fallback_value_text("display", "repr"), "display");
    }

    #[test]
    fn uses_repr_when_display_empty() {
        assert_eq!(fallback_value_text("", "repr"), "repr");
    }
}

// ─── BundleInterface conversion tests ────────────────────────────────────────

#[cfg(test)]
mod bundle_interface_conversion_tests {
    use super::*;
    use crate::python_interface::{
        PyLong_FromUnsignedLongLong, PyObject_SetAttrString, PyUnicode_FromString, Py_eval_input,
    };

    /// Helper: create a minimal `BundleInterface` with null pointer fields.
    /// Safe for testing null-object paths that don't dereference inner fields.
    fn null_bundle() -> BundleInterface {
        BundleInterface {
            inner: Arc::new(BundleInterfaceInner {
                python_interpreter: SubInterpreter::null(),
                p_global: std::ptr::null_mut(),
                p_bundle_module: std::ptr::null_mut(),
                json_module: std::ptr::null_mut(),
                traceback_module: std::ptr::null_mut(),
                bundle_hash: "test-bundle".to_string(),
                thread_scope_error: None,
            }),
        }
    }

    #[test]
    fn to_bool_returns_false_for_null_pointer() {
        unsafe {
            assert!(!BundleInterface::to_bool(std::ptr::null_mut()));
        }
    }

    #[test]
    fn to_bool_returns_true_for_true_singleton() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state for the Python
        // singleton lookup performed by `to_bool`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            assert!(BundleInterface::to_bool(my_py_true_struct()));
        }
    }

    #[test]
    fn to_bool_returns_false_for_non_true_object() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state for the Python
        // singleton lookup performed by `to_bool`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            assert!(!BundleInterface::to_bool(my_py_none_struct()));
        }
    }

    #[test]
    fn to_uint64_returns_zero_for_null_pointer() {
        unsafe {
            assert_eq!(BundleInterface::to_uint64(std::ptr::null_mut()), 0);
        }
    }

    #[test]
    fn to_string_py_returns_empty_for_null_pointer() {
        unsafe {
            assert_eq!(BundleInterface::to_string_py(std::ptr::null_mut()), "");
        }
    }

    #[test]
    fn to_string_py_returns_empty_and_clears_error_for_non_str_object() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `to_string_py` and `PyErr_Clear`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let int_obj = PyLong_FromUnsignedLongLong(42);
            assert!(!int_obj.is_null(), "int object should be created");
            assert_eq!(BundleInterface::to_string_py(int_obj), "");
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyUnicode_AsUTF8 must be cleared"
            );
            Py_DecRef(int_obj);
        }
    }

    #[test]
    fn to_uint64_returns_zero_and_clears_error_for_non_int_object() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `to_uint64` and `PyErr_Clear`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let str_obj = PyUnicode_FromString(c"not an int".as_ptr());
            assert!(!str_obj.is_null(), "str object should be created");
            assert_eq!(BundleInterface::to_uint64(str_obj), 0);
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyLong_AsUnsignedLongLong must be cleared"
            );
            Py_DecRef(str_obj);
        }
    }

    #[test]
    fn dispose_object_is_safe_with_null_pointer() {
        // Should not crash or dereference null
        unsafe {
            BundleInterface::dispose_object(std::ptr::null_mut());
        }
    }

    #[test]
    fn json_dumps_returns_null_string_for_null_pointer() {
        let bundle = null_bundle();
        unsafe {
            let result = bundle.json_dumps(std::ptr::null_mut());
            assert_eq!(result.unwrap(), "null");
        }
    }

    /// Converter stub that always fails (returns NULL) to exercise the
    /// failure branch of `extract_value_string`.
    // SAFETY: Test-only; ignores `obj` and returns NULL without touching the
    // Python error indicator.
    unsafe fn always_null_converter(_obj: *mut PyObject) -> *mut PyObject {
        std::ptr::null_mut()
    }

    #[test]
    fn extract_value_string_returns_empty_for_null_value() {
        // SAFETY: The NULL-value branch returns before any Python C-API call,
        // so no GIL/thread state is required.
        unsafe {
            assert_eq!(
                extract_value_string(std::ptr::null_mut(), PyObject_Repr),
                ""
            );
        }
    }

    #[test]
    fn extract_value_string_returns_empty_when_converter_fails() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `extract_value_string` and `swallow_python_error`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let value = my_py_true_struct();
            assert_eq!(extract_value_string(value, always_null_converter), "");
        }
    }

    /// Converter stub that returns a non-unicode `PyLong` so that
    /// `PyUnicode_AsUTF8` fails inside `extract_value_string`, exercising the
    /// `PyErr_Clear()` + empty-string branch.
    // SAFETY: Test-only; returns a new reference to a PyLong without touching
    // the Python error indicator.
    unsafe fn non_unicode_converter(_obj: *mut PyObject) -> *mut PyObject {
        PyLong_FromUnsignedLongLong(42)
    }

    #[test]
    fn extract_value_string_returns_empty_when_unicode_as_utf8_fails() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `extract_value_string` and `PyErr_Clear`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let value = my_py_true_struct();
            assert_eq!(extract_value_string(value, non_unicode_converter), "");
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyUnicode_AsUTF8 must be cleared"
            );
        }
    }

    #[test]
    fn extract_type_name_returns_unknown_when_unicode_as_utf8_fails() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `extract_type_name` and `PyErr_Clear`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            // Create a fresh class instance whose `__name__` attribute is a
            // non-unicode int, so `PyUnicode_AsUTF8` fails inside
            // `extract_type_name`.
            let globals = PyDict_New();
            assert!(!globals.is_null(), "globals dict should be created");
            assert_eq!(
                PyDict_SetItemString(globals, c"__builtins__".as_ptr(), PyEval_GetBuiltins()),
                0,
                "setting __builtins__ should succeed"
            );
            let code = c"type('_T', (), {})()";
            let instance = PyRun_StringFlags(
                code.as_ptr(),
                Py_eval_input,
                globals,
                globals,
                std::ptr::null_mut(),
            );
            assert!(!instance.is_null(), "class instance should be created");
            let int_obj = PyLong_FromUnsignedLongLong(42);
            assert!(!int_obj.is_null(), "int object should be created");
            assert_eq!(
                PyObject_SetAttrString(instance, c"__name__".as_ptr(), int_obj),
                0,
                "setting __name__ should succeed"
            );
            assert_eq!(extract_type_name(instance), "unknown");
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyUnicode_AsUTF8 must be cleared"
            );
            Py_DecRef(int_obj);
            Py_DecRef(instance);
            Py_DecRef(globals);
        }
    }

    /// `print_last_python_exception` must return silently when no Python
    /// exception is active (the `extype.is_null()` early-return branch),
    /// without dereferencing the null `traceback_module` and without leaving
    /// a stale error set.
    #[test]
    fn print_last_python_exception_returns_silently_when_no_exception_active() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state for PyErr_Fetch
        // and PyErr_Occurred. The null_bundle's traceback_module is never
        // dereferenced because the early-return branch triggers first.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            PyErr_Clear();
            let bundle = null_bundle();
            bundle.print_last_python_exception();
            assert!(
                PyErr_Occurred().is_null(),
                "no error indicator should be left after the no-exception early return"
            );
        }
    }

    /// `extract_type_name` must return `"unknown"` when the `__name__`
    /// attribute lookup itself fails (e.g. `extype` is not a type), swallowing
    /// the raised `AttributeError` so no stale error is left for the caller.
    #[test]
    fn extract_type_name_returns_unknown_when_name_lookup_fails() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `extract_type_name` and `swallow_python_error`.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            // An int has no `__name__` attribute, so `PyObject_GetAttrString`
            // fails and `extract_type_name` must swallow the error.
            let int_obj = PyLong_FromUnsignedLongLong(42);
            assert!(!int_obj.is_null(), "int object should be created");
            assert_eq!(extract_type_name(int_obj), "unknown");
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyObject_GetAttrString must be cleared"
            );
            Py_DecRef(int_obj);
        }
    }
}

// ─── set_exception_value_slot tests ──────────────────────────────────────────

#[cfg(test)]
mod set_exception_value_slot_tests {
    use super::*;
    use crate::python_interface::{
        set_py_tuple_set_item_override, PyTupleSetItemFn, PyTuple_GetItem, PyTuple_Size, Py_ssize_t,
    };
    use std::os::raw::c_int;

    /// RAII guard that installs a `py_tuple_set_item` override for the duration
    /// of a test and restores the previous override on drop.
    struct TupleSetItemOverrideGuard(Option<PyTupleSetItemFn>);

    impl TupleSetItemOverrideGuard {
        fn install(f: PyTupleSetItemFn) -> Self {
            Self(set_py_tuple_set_item_override(Some(f)))
        }
    }

    impl Drop for TupleSetItemOverrideGuard {
        fn drop(&mut self) {
            set_py_tuple_set_item_override(self.0);
        }
    }

    /// Override that fails `PyTuple_SetItem` only for the size-2 `eo_args`
    /// tuple at index 1 when the item is `Py_None` (the NULL-value slot).
    // SAFETY: Test-only; `tuple`/`item` are live objects from the caller.
    unsafe fn fail_none_item(tuple: *mut PyObject, pos: Py_ssize_t, item: *mut PyObject) -> c_int {
        if PyTuple_Size(tuple) == 2 && pos == 1 && item == my_py_none_struct() {
            Py_DecRef(item);
            -1
        } else {
            PyTuple_SetItem(tuple, pos, item)
        }
    }

    /// Override that fails `PyTuple_SetItem` only for the size-2 `eo_args`
    /// tuple at index 1 when the item is not `Py_None` (the value slot).
    // SAFETY: Test-only; `tuple`/`item` are live objects from the caller.
    unsafe fn fail_non_none_item(
        tuple: *mut PyObject,
        pos: Py_ssize_t,
        item: *mut PyObject,
    ) -> c_int {
        if PyTuple_Size(tuple) == 2 && pos == 1 && item != my_py_none_struct() {
            Py_DecRef(item);
            -1
        } else {
            PyTuple_SetItem(tuple, pos, item)
        }
    }

    /// `set_exception_value_slot` must store `Py_None` when `value` is NULL
    /// (the `value.is_null()` branch) and report success.
    #[test]
    fn passes_none_for_null_value() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let eo_args = PyTuple_New(2);
            assert!(!eo_args.is_null(), "PyTuple_New should succeed");
            let eo_func = my_py_true_struct();
            let ok = set_exception_value_slot(eo_args, std::ptr::null_mut(), eo_func);
            assert!(ok, "NULL value should store Py_None and succeed");
            let stored = PyTuple_GetItem(eo_args, 1);
            assert_eq!(stored, my_py_none_struct(), "Py_None should be stored");
            Py_DecRef(eo_args);
        }
    }

    /// `set_exception_value_slot` must store a non-NULL value (the non-NULL
    /// branch) and report success.
    #[test]
    fn stores_value_for_non_null_value() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let eo_args = PyTuple_New(2);
            assert!(!eo_args.is_null(), "PyTuple_New should succeed");
            let eo_func = my_py_true_struct();
            let value = my_py_true_struct();
            Py_IncRef(value); // SetItem steals this ref into the tuple
            let ok = set_exception_value_slot(eo_args, value, eo_func);
            assert!(ok, "non-NULL value should be stored");
            let stored = PyTuple_GetItem(eo_args, 1);
            assert_eq!(stored, value, "value should be stored");
            Py_DecRef(eo_args);
        }
    }

    /// `set_exception_value_slot` must log the marker and return `false` when
    /// the `Py_None` slot `SetItem` fails (the NULL-value failure branch).
    #[test]
    fn handles_none_set_item_failure() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let _override = TupleSetItemOverrideGuard::install(fail_none_item);
            let eo_args = PyTuple_New(2);
            assert!(!eo_args.is_null(), "PyTuple_New should succeed");
            let eo_func = my_py_true_struct();
            let ok = set_exception_value_slot(eo_args, std::ptr::null_mut(), eo_func);
            assert!(!ok, "None SetItem failure should return false");
        }
    }

    /// `set_exception_value_slot` must log the marker and return `false` when
    /// the value slot `SetItem` fails (the non-NULL-value failure branch).
    #[test]
    fn handles_value_set_item_failure() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let _override = TupleSetItemOverrideGuard::install(fail_non_none_item);
            let eo_args = PyTuple_New(2);
            assert!(!eo_args.is_null(), "PyTuple_New should succeed");
            let eo_func = my_py_true_struct();
            let value = my_py_true_struct();
            let ok = set_exception_value_slot(eo_args, value, eo_func);
            assert!(!ok, "value SetItem failure should return false");
        }
    }
}

// ─── append_bundle_path_to_sys_path tests ────────────────────────────────────

#[cfg(test)]
mod append_bundle_path_to_sys_path_tests {
    use super::*;
    use crate::bundle_manager::BundleManager;
    use crate::tests::fixtures::bundle_fixture::BundleFixture;
    use uuid::Uuid;

    /// Load a real bundle so the sub-interpreter has a live `sys.path`.
    fn load_test_bundle() -> BundleInterface {
        crate::tests::init_python_global();
        let fixture = BundleFixture::new();
        let bundle_hash = Uuid::new_v4().to_string();
        BundleManager::initialize(fixture.get_bundle_path().to_string_lossy().to_string());
        fixture.write_raw_script(
            &bundle_hash,
            "def submit(details, job_data):\n    return {}\n",
        );
        BundleManager::singleton()
            .load_bundle(&bundle_hash)
            .expect("bundle should load")
    }

    #[test]
    fn returns_err_on_null_sys_path() {
        let bundle = load_test_bundle();
        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let result = BundleInterface::append_bundle_path_to_sys_path(
                std::ptr::null_mut(),
                Path::new("/some/bundle/path"),
            );
            assert!(
                result.is_err(),
                "NULL sys.path should make append return Err"
            );
        }
    }

    #[test]
    fn returns_err_when_pylist_append_fails() {
        let bundle = load_test_bundle();
        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let p_not_a_list = PyTuple_New(0);
            assert!(!p_not_a_list.is_null(), "PyTuple_New should succeed");
            let result = BundleInterface::append_bundle_path_to_sys_path(
                p_not_a_list,
                Path::new("/some/bundle/path"),
            );
            Py_DecRef(p_not_a_list);
            assert!(
                result.is_err(),
                "non-list sys.path should make append return Err"
            );
        }
    }

    #[test]
    fn appends_bundle_path_on_success() {
        let bundle = load_test_bundle();
        let _guard = PYTHON_MUTEX.lock();
        unsafe {
            let _scope = bundle.thread_scope().expect("thread scope");
            let p_path = PySys_GetObject(c"path".as_ptr());
            assert!(!p_path.is_null(), "sys.path should exist");
            let result = BundleInterface::append_bundle_path_to_sys_path(
                p_path,
                Path::new("/some/bundle/path"),
            );
            assert!(result.is_ok(), "append should succeed: {result:?}");
        }
    }
}

// ─── log_python_lines tests ──────────────────────────────────────────────────

#[cfg(test)]
mod log_python_lines_tests {
    use super::*;
    use crate::python_interface::{PyLong_FromUnsignedLongLong, Py_eval_input};
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct VecWriter(Arc<Mutex<Vec<u8>>>);

    impl VecWriter {
        fn new() -> Self {
            Self::default()
        }

        fn into_string(self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap_or_default()
        }
    }

    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            VecWriterGuard(self.0.clone())
        }
    }

    struct VecWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for VecWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `f` with a thread-local tracing subscriber that captures all
    /// `INFO`-and-above events into a `String`. Returns the captured log.
    fn capture_logs<F: FnOnce()>(f: F) -> String {
        let writer = VecWriter::new();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_target(true)
            .with_level(true)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        writer.into_string()
    }

    /// `log_python_lines` must iterate a Python list of strings and log each
    /// element via `error!`.
    #[test]
    fn logs_each_string_in_list() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `log_python_lines` and the Python C-API calls used
        // to build the input list.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let globals = PyDict_New();
            assert!(!globals.is_null(), "globals dict should be created");
            assert_eq!(
                PyDict_SetItemString(globals, c"__builtins__".as_ptr(), PyEval_GetBuiltins()),
                0,
                "setting __builtins__ should succeed"
            );
            let code = c"['line one', 'line two', 'line three']";
            let list = PyRun_StringFlags(
                code.as_ptr(),
                Py_eval_input,
                globals,
                globals,
                std::ptr::null_mut(),
            );
            assert!(!list.is_null(), "list literal should evaluate");
            let logs = capture_logs(|| log_python_lines(list));
            Py_DecRef(list);
            Py_DecRef(globals);
            for line in ["line one", "line two", "line three"] {
                assert!(
                    logs.contains(line),
                    "expected '{line}' to be logged via error!, got:\n{logs}"
                );
            }
        }
    }

    /// `log_python_lines` must return silently on NULL input without touching
    /// the Python C-API or emitting any log events.
    #[test]
    fn returns_silently_on_null_input() {
        crate::tests::init_python_global();
        // SAFETY: The NULL branch returns before any Python C-API call, so no
        // GIL/thread state is required.
        unsafe {
            let logs = capture_logs(|| log_python_lines(std::ptr::null_mut()));
            assert!(
                logs.is_empty(),
                "NULL input should log nothing, got:\n{logs}"
            );
        }
    }

    /// `log_python_lines` must swallow the `TypeError` raised by
    /// `PyObject_GetIter` on a non-iterable object and log nothing, leaving no
    /// stale error set for the next Python C-API call.
    #[test]
    fn returns_silently_on_non_iterable_input() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `log_python_lines` and the Python C-API calls used
        // to build the input int.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let non_iterable = PyLong_FromUnsignedLongLong(42);
            assert!(!non_iterable.is_null(), "int object should be created");
            let logs = capture_logs(|| log_python_lines(non_iterable));
            assert!(
                logs.is_empty(),
                "non-iterable input should log nothing, got:\n{logs}"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyObject_GetIter must be swallowed"
            );
            Py_DecRef(non_iterable);
        }
    }

    /// `log_python_lines` must swallow the exception raised by `PyIter_Next`
    /// (a non-StopIteration error from the iterator), stop iterating, and
    /// leave no stale error set for the next Python C-API call.
    #[test]
    fn swallows_error_and_stops_when_iterator_raises() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `log_python_lines` and the Python C-API calls used
        // to build the input generator.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let globals = PyDict_New();
            assert!(!globals.is_null(), "globals dict should be created");
            assert_eq!(
                PyDict_SetItemString(globals, c"__builtins__".as_ptr(), PyEval_GetBuiltins()),
                0,
                "setting __builtins__ should succeed"
            );
            let code = c"(1 / 0 for _ in [1])";
            let gen = PyRun_StringFlags(
                code.as_ptr(),
                Py_eval_input,
                globals,
                globals,
                std::ptr::null_mut(),
            );
            assert!(!gen.is_null(), "generator expression should evaluate");
            let logs = capture_logs(|| log_python_lines(gen));
            assert!(
                logs.is_empty(),
                "iterator raising on next() should log nothing, got:\n{logs}"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyIter_Next must be swallowed"
            );
            Py_DecRef(gen);
            Py_DecRef(globals);
        }
    }

    /// `log_python_lines` must swallow the `TypeError` raised by
    /// `PyUnicode_AsUTF8` on a non-str list element, log nothing for that
    /// element, and leave no stale error set for the next Python C-API call.
    #[test]
    fn swallows_error_and_logs_nothing_for_non_str_element() {
        crate::tests::init_python_global();
        // SAFETY: PYTHON_MUTEX is held and a ThreadScope on the main
        // interpreter provides a valid current thread state, satisfying the
        // preconditions of `log_python_lines` and the Python C-API calls used
        // to build the input list.
        unsafe {
            let _guard = PYTHON_MUTEX.lock();
            let interp = (*get_main_ts()).interp;
            let _scope = ThreadScope::new(interp).expect("thread scope should be created");
            let globals = PyDict_New();
            assert!(!globals.is_null(), "globals dict should be created");
            assert_eq!(
                PyDict_SetItemString(globals, c"__builtins__".as_ptr(), PyEval_GetBuiltins()),
                0,
                "setting __builtins__ should succeed"
            );
            let code = c"[42]";
            let list = PyRun_StringFlags(
                code.as_ptr(),
                Py_eval_input,
                globals,
                globals,
                std::ptr::null_mut(),
            );
            assert!(!list.is_null(), "list literal should evaluate");
            let logs = capture_logs(|| log_python_lines(list));
            assert!(
                logs.is_empty(),
                "list with non-str element should log nothing, got:\n{logs}"
            );
            assert!(
                PyErr_Occurred().is_null(),
                "stale error from PyUnicode_AsUTF8 must be swallowed"
            );
            Py_DecRef(list);
            Py_DecRef(globals);
        }
    }
}
