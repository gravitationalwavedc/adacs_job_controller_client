#![allow(non_snake_case)]

use libloading::{Library, Symbol};
use parking_lot::Mutex;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Arc, OnceLock};

// ─── Python C-API type aliases ───────────────────────────────────────────────
pub type PyObject = c_void;
pub type PyInterpreterState = c_void;
pub type Py_ssize_t = isize;

/// Minimal repr of `PyThreadState` – only the fields we actually dereference.
/// The real struct has many more fields, but we only need `interp`.
#[repr(C)]
pub struct PyThreadState {
    pub prev: *mut PyThreadState,
    pub next: *mut PyThreadState,
    pub interp: *mut PyInterpreterState,
}

#[repr(C)]
pub struct PyObject_Head {
    pub ob_refcnt: Py_ssize_t,
    pub ob_type: *mut c_void,
}

#[repr(C)]
pub struct PyModuleDef_Base {
    pub ob_base: PyObject_Head,
    pub m_init: Option<unsafe extern "C" fn() -> *mut PyObject>,
    pub m_index: Py_ssize_t,
    pub m_copy: *mut PyObject,
}

#[repr(C)]
#[allow(clippy::struct_field_names)]
pub struct PyModuleDef {
    pub m_base: PyModuleDef_Base,
    pub m_name: *const c_char,
    pub m_doc: *const c_char,
    pub m_size: Py_ssize_t,
    pub m_methods: *mut PyMethodDef,
    pub m_slots: *mut c_void,
    pub m_traverse: *mut c_void,
    pub m_clear: *mut c_void,
    pub m_free: *mut c_void,
}

#[repr(C)]
#[allow(clippy::struct_field_names)]
pub struct PyMethodDef {
    pub ml_name: *const c_char,
    pub ml_meth: Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject>,
    pub ml_flags: c_int,
    pub ml_doc: *const c_char,
}

pub const METH_VARARGS: c_int = 0x0001;
pub const Py_file_input: c_int = 257;
pub const PYTHON_API_VERSION: c_int = 1013;

pub type PyGILState_STATE = c_int;
pub const PY_GILSTATE_LOCKED: PyGILState_STATE = 0;
#[allow(dead_code)]
pub const PY_GILSTATE_UNLOCKED: PyGILState_STATE = 1;

// ─── py_wrap! macro ──────────────────────────────────────────────────────────
// Each wrapped function looks up its symbol in the dynamically loaded libpython.
macro_rules! py_wrap {
    ($name:ident, ($($arg:ident: $typ:ty),*) -> $ret:ty) => {
        #[allow(non_snake_case)]
        pub unsafe fn $name($($arg: $typ),*) -> $ret {
            let lib = get_python_lib();
            let symbol: Symbol<unsafe extern "C" fn($($arg: $typ),*) -> $ret> =
                lib.get(CString::new(stringify!($name)).unwrap().as_bytes()).unwrap();
            symbol($($arg),*)
        }
    };
}

// ─── Global state ────────────────────────────────────────────────────────────
static PY_LIB: OnceLock<Arc<Library>> = OnceLock::new();

/// Global mutex that serialises ALL Python C-API access from Rust.
/// Mirrors the C++ `static std::shared_mutex mutex_` used throughout.
pub static PYTHON_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
pub struct ThreadStatePtr(pub *mut PyThreadState);
unsafe impl Send for ThreadStatePtr {}
unsafe impl Sync for ThreadStatePtr {}

static MAIN_TS: OnceLock<ThreadStatePtr> = OnceLock::new();
static INIT_PYTHON: std::sync::Once = std::sync::Once::new();

// ─── Library loading ─────────────────────────────────────────────────────────
pub fn load_python_library(path: &str) {
    if PY_LIB.get().is_some() {
        return;
    }
    // RTLD_NOW | RTLD_GLOBAL – matches the C++ dlopen flags exactly.
    let lib = unsafe {
        let flags = libc::RTLD_NOW | libc::RTLD_GLOBAL;
        let c_path = CString::new(path).expect("invalid library path");
        let handle = libc::dlopen(c_path.as_ptr(), flags);
        if handle.is_null() {
            let err = std::ffi::CStr::from_ptr(libc::dlerror());
            panic!("Failed to dlopen libpython: {}", err.to_string_lossy());
        }
        // Now wrap via libloading so py_wrap! can use it.
        Library::new(path).expect("Failed to load libpython via libloading")
    };
    let _ = PY_LIB.set(Arc::new(lib));
}

pub fn get_python_lib() -> Arc<Library> {
    PY_LIB
        .get()
        .expect("Python library not loaded – call load_python_library() first")
        .clone()
}

pub fn get_main_ts() -> *mut PyThreadState {
    MAIN_TS.get().expect("Python not initialized").0
}

// ─── Wrapped Python C-API functions ──────────────────────────────────────────
py_wrap!(Py_Initialize, () -> ());
py_wrap!(Py_NewInterpreter, () -> *mut PyThreadState);
py_wrap!(Py_EndInterpreter, (ts: *mut PyThreadState) -> ());
py_wrap!(PyThreadState_Get, () -> *mut PyThreadState);
py_wrap!(PyThreadState_Swap, (state: *mut PyThreadState) -> *mut PyThreadState);
py_wrap!(PyImport_AppendInittab, (name: *const c_char, init_func: Option<unsafe extern "C" fn() -> *mut PyObject>) -> c_int);
py_wrap!(PyImport_ImportModule, (name: *const c_char) -> *mut PyObject);
py_wrap!(PyDict_New, () -> *mut PyObject);
py_wrap!(PyDict_SetItemString, (dict: *mut PyObject, key: *const c_char, item: *mut PyObject) -> c_int);
py_wrap!(PyEval_GetBuiltins, () -> *mut PyObject);
py_wrap!(PyObject_GetAttrString, (obj: *mut PyObject, name: *const c_char) -> *mut PyObject);
py_wrap!(PyObject_CallObject, (callable: *mut PyObject, args: *mut PyObject) -> *mut PyObject);
py_wrap!(PyObject_Repr, (obj: *mut PyObject) -> *mut PyObject);
py_wrap!(PyObject_GetIter, (obj: *mut PyObject) -> *mut PyObject);
py_wrap!(PyIter_Next, (obj: *mut PyObject) -> *mut PyObject);
py_wrap!(PyTuple_New, (len: Py_ssize_t) -> *mut PyObject);
py_wrap!(PyTuple_SetItem, (tuple: *mut PyObject, pos: Py_ssize_t, item: *mut PyObject) -> c_int);
py_wrap!(PyTuple_GetItem, (tuple: *mut PyObject, pos: Py_ssize_t) -> *mut PyObject);
py_wrap!(Py_IncRef, (obj: *mut PyObject) -> ());
py_wrap!(Py_DecRef, (obj: *mut PyObject) -> ());
py_wrap!(PyThreadState_New, (interp: *mut PyInterpreterState) -> *mut PyThreadState);
py_wrap!(PyEval_RestoreThread, (state: *mut PyThreadState) -> ());
py_wrap!(PyEval_InitThreads, () -> ());
py_wrap!(PyThreadState_Clear, (state: *mut PyThreadState) -> ());
py_wrap!(PyThreadState_DeleteCurrent, () -> ());
py_wrap!(PyEval_SaveThread, () -> *mut PyThreadState);
py_wrap!(PyUnicode_AsUTF8, (obj: *mut PyObject) -> *const c_char);
py_wrap!(PyUnicode_FromString, (obj: *const c_char) -> *mut PyObject);
py_wrap!(PyErr_Occurred, () -> *mut PyObject);
py_wrap!(PyErr_Fetch, (extype: *mut *mut PyObject, value: *mut *mut PyObject, traceback: *mut *mut PyObject) -> ());
py_wrap!(PyErr_Print, () -> ());
py_wrap!(PyCallable_Check, (callable: *mut PyObject) -> c_int);
py_wrap!(PyObject_IsTrue, (obj: *mut PyObject) -> c_int);
py_wrap!(PyObject_Str, (obj: *mut PyObject) -> *mut PyObject);
py_wrap!(PySys_GetObject, (obj: *const c_char) -> *mut PyObject);
py_wrap!(PyList_Append, (list: *mut PyObject, item: *mut PyObject) -> c_int);
py_wrap!(PyModule_Create2, (module_def: *mut PyModuleDef, apiver: c_int) -> *mut PyObject);
py_wrap!(PyLong_FromUnsignedLongLong, (value: u64) -> *mut PyObject);
py_wrap!(PyLong_AsUnsignedLongLong, (obj: *mut PyObject) -> u64);
py_wrap!(PyErr_NewException, (name: *const c_char, base: *mut PyObject, dict: *mut PyObject) -> *mut PyObject);
py_wrap!(PyModule_AddObject, (module: *mut PyObject, name: *const c_char, value: *mut PyObject) -> c_int);
py_wrap!(PyErr_SetString, (type_: *mut PyObject, message: *const c_char) -> ());
py_wrap!(PyRun_StringFlags, (code: *const c_char, start: c_int, globals: *mut PyObject, locals: *mut PyObject, flags: *mut c_void) -> *mut PyObject);

// ─── Convenience helpers ─────────────────────────────────────────────────────
pub unsafe fn Py_XDECREF(obj: *mut PyObject) {
    if !obj.is_null() {
        Py_DecRef(obj);
    }
}

pub unsafe fn my_py_none_struct() -> *mut PyObject {
    let lib = get_python_lib();
    let symbol: Symbol<*mut PyObject> = lib.get(b"_Py_NoneStruct\0").unwrap();
    *symbol
}

pub unsafe fn my_py_true_struct() -> *mut PyObject {
    let lib = get_python_lib();
    let symbol: Symbol<*mut PyObject> = lib.get(b"_Py_TrueStruct\0").unwrap();
    *symbol
}

pub unsafe fn MyPy_IsNone(obj: *mut PyObject) -> bool {
    obj == my_py_none_struct()
}

// ─── GIL hook stubs (called by subhook) ──────────────────────────────────────
// These replace the real PyGILState_Ensure / PyGILState_Release in libpython
// at runtime via binary patching, exactly as the C++ code does.
#[unsafe(no_mangle)]
pub extern "C" fn myPyGILState_Ensure() -> PyGILState_STATE {
    tracing::info!("myPyGILState_Ensure called");
    PY_GILSTATE_LOCKED
}

#[unsafe(no_mangle)]
pub extern "C" fn myPyGILState_Release(_state: PyGILState_STATE) {
    tracing::info!("myPyGILState_Release called");
}

// ─── subhook FFI bindings ────────────────────────────────────────────────────
include!(concat!(env!("OUT_DIR"), "/subhook_bindings.rs"));

/// Install subhook-based patches on `PyGILState_Ensure` and `PyGILState_Release`.
/// Mirrors the C++ `PythonInterface::initPython()` hook installation exactly.
unsafe fn install_gil_hooks() {
    let lib = get_python_lib();

    let p_ensure: Symbol<*mut c_void> = lib.get(b"PyGILState_Ensure").unwrap();
    let p_release: Symbol<*mut c_void> = lib.get(b"PyGILState_Release").unwrap();

    let hook_ensure = subhook_new(
        *p_ensure,
        myPyGILState_Ensure as *mut c_void,
        subhook_flags_SUBHOOK_64BIT_OFFSET,
    );
    assert!(
        subhook_install(hook_ensure) >= 0,
        "PyGILState_Ensure redirection failed to install"
    );

    let hook_release = subhook_new(
        *p_release,
        myPyGILState_Release as *mut c_void,
        subhook_flags_SUBHOOK_64BIT_OFFSET,
    );
    assert!(
        subhook_install(hook_release) >= 0,
        "myPyGILState_Release redirection failed to install"
    );

    tracing::info!("GIL hooks installed successfully");
}

// ─── Python initialisation ───────────────────────────────────────────────────
// Mirrors the C++ PythonInterface::initPython() exactly:
//   1. dlopen(lib, RTLD_NOW | RTLD_GLOBAL)
//   2. Install subhook GIL patches
//   3. PyImport_AppendInittab for _bundledb and _bundlelogging
//   4. Py_Initialize()
//   5. PyEval_InitThreads()
//   6. (caller must save the main thread state afterwards)
//
// NOTE: PyImport_AppendInittab calls must happen BEFORE this function is called,
// and the library must already be loaded.
pub fn init_python() {
    INIT_PYTHON.call_once(|| {
        unsafe {
            // Install GIL hooks (subhook patches)
            install_gil_hooks();

            // Initialise the interpreter
            Py_Initialize();
            PyEval_InitThreads();

            // Save the main thread state so we can restore it later
            let ts = PyThreadState_Get();
            let _ = MAIN_TS.set(ThreadStatePtr(ts));
        }
    });
}

// ─── SubInterpreter ──────────────────────────────────────────────────────────
// Exact port of C++ PythonInterface::SubInterpreter.
//
// Construction:
//   1. Save+restore the current thread state (RestoreThreadStateScope)
//   2. Call Py_NewInterpreter() to create a new sub-interpreter
//
// Destruction:
//   1. Swap to the sub-interpreter's thread state
//   2. Call Py_EndInterpreter()
//   3. Restore the previous thread state
pub struct SubInterpreter {
    ts: *mut PyThreadState,
}

unsafe impl Send for SubInterpreter {}
unsafe impl Sync for SubInterpreter {}

impl SubInterpreter {
    /// Creates a new sub-interpreter. MUST be called with the GIL held
    /// (i.e., with a valid current thread state).
    pub unsafe fn new() -> Self {
        // RestoreThreadStateScope – save current ts, restore on drop
        let saved_ts = PyThreadState_Get();

        let ts = Py_NewInterpreter();
        assert!(!ts.is_null(), "Py_NewInterpreter failed");

        // Restore the original thread state (like C++ RestoreThreadStateScope destructor)
        PyThreadState_Swap(saved_ts);

        SubInterpreter { ts }
    }

    /// Get the interpreter state pointer (for creating `ThreadScopes`)
    pub unsafe fn interp(&self) -> *mut PyInterpreterState {
        (*self.ts).interp
    }
}

impl Drop for SubInterpreter {
    fn drop(&mut self) {
        unsafe {
            if !self.ts.is_null() {
                // SwapThreadStateScope – swap to sub-interp, end it, swap back
                let old_ts = PyThreadState_Swap(self.ts);
                Py_EndInterpreter(self.ts);
                PyThreadState_Swap(old_ts);
            }
        }
    }
}

// ─── ThreadScope ─────────────────────────────────────────────────────────────
// Exact port of C++ SubInterpreter::ThreadScope (ThreadState + SwapThreadStateScope).
//
// Creates a new thread state for the given interpreter, makes it current,
// and on drop: clears the thread state and deletes it (via PyThreadState_DeleteCurrent).
pub struct ThreadScope {
    _ts: *mut PyThreadState,
    saved_ts: *mut PyThreadState,
}

impl ThreadScope {
    /// Create a new `ThreadScope` for the given interpreter.
    /// This is the equivalent of C++ `SubInterpreter::ThreadScope`.
    pub unsafe fn new(interp: *mut PyInterpreterState) -> Self {
        // ThreadState constructor: create new ts, restore thread (acquires GIL for this ts)
        let ts = PyThreadState_New(interp);
        PyEval_RestoreThread(ts);

        // SwapThreadStateScope: swap to the new ts, save the one RestoreThread set
        let saved_ts = PyThreadState_Swap(ts);

        ThreadScope { _ts: ts, saved_ts }
    }
}

impl Drop for ThreadScope {
    fn drop(&mut self) {
        unsafe {
            // ~SwapThreadStateScope: restore the saved ts
            let ts = PyThreadState_Swap(self.saved_ts);
            // ~ThreadState: clear and delete the thread state
            PyThreadState_Clear(ts);
            PyThreadState_DeleteCurrent();
        }
    }
}
