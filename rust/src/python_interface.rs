use std::os::raw::{c_char, c_int, c_void};
use std::ffi::{CString};
use libloading::{Library, Symbol};
use std::sync::{Arc, OnceLock};
use parking_lot::Mutex;

pub type PyObject = c_void;
pub type PyInterpreterState = c_void;
pub type Py_ssize_t = isize;

#[repr(C)]
pub struct PyThreadState {
    pub prev: *mut PyThreadState,
    pub next: *mut PyThreadState,
    pub interp: *mut PyInterpreterState,
}

#[repr(C)]
pub struct PyModuleDef_Base {
    pub ob_base: PyObject_Head,
    pub m_init: Option<unsafe extern "C" fn() -> *mut PyObject>,
    pub m_index: Py_ssize_t,
    pub m_copy: *mut PyObject,
}

#[repr(C)]
pub struct PyObject_Head {
    pub ob_refcnt: Py_ssize_t,
    pub ob_type: *mut c_void,
}

#[repr(C)]
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
pub struct PyMethodDef {
    pub ml_name: *const c_char,
    pub ml_meth: Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject>,
    pub ml_flags: c_int,
    pub ml_doc: *const c_char,
}

pub const METH_VARARGS: c_int = 0x0001;
pub const PYTHON_API_VERSION: c_int = 1013;

pub type PyGILState_STATE = c_int;
pub const PyGILState_LOCKED: PyGILState_STATE = 0;
pub const PyGILState_UNLOCKED: PyGILState_STATE = 1;

macro_rules! py_wrap {
    ($name:ident, ($($arg:ident: $typ:ty),*) -> $ret:ty) => {
        pub unsafe fn $name($($arg: $typ),*) -> $ret {
            let lib = get_python_lib();
            let symbol: Symbol<unsafe extern "C" fn($($arg: $typ),*) -> $ret> = lib.get(CString::new(stringify!($name)).unwrap().as_bytes()).unwrap();
            symbol($($arg),*)
        }
    };
}

static PY_LIB: OnceLock<Arc<Library>> = OnceLock::new();
pub static PYTHON_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
pub struct ThreadStatePtr(pub *mut PyThreadState);
unsafe impl Send for ThreadStatePtr {}
unsafe impl Sync for ThreadStatePtr {}

static MAIN_TS: OnceLock<ThreadStatePtr> = OnceLock::new();
static INIT_PYTHON: std::sync::Once = std::sync::Once::new();

pub fn load_python_library(path: &str) {
    if PY_LIB.get().is_some() { return; }
    let lib = unsafe { Library::new(path).expect("Failed to load libpython") };
    let _ = PY_LIB.set(Arc::new(lib));
}

pub fn get_python_lib() -> Arc<Library> {
    PY_LIB.get_or_init(|| {
        let lib = unsafe { Library::new("/usr/lib/x86_64-linux-gnu/libpython3.11.so").expect("Failed to load libpython fallback") };
        Arc::new(lib)
    }).clone()
}

pub fn get_main_ts() -> *mut PyThreadState {
    MAIN_TS.get().expect("Python not initialized").0
}

py_wrap!(Py_Initialize, () -> ());
py_wrap!(Py_Finalize, () -> ());
py_wrap!(Py_NewInterpreter, () -> *mut PyThreadState);
py_wrap!(Py_EndInterpreter, (ts: *mut PyThreadState) -> ());
py_wrap!(PyThreadState_Get, () -> *mut PyThreadState);
py_wrap!(PyThreadState_Swap, (state: *mut PyThreadState) -> *mut PyThreadState);
py_wrap!(PyImport_AppendInittab, (name: *const c_char, init_func: Option<unsafe extern "C" fn() -> *mut PyObject>) -> c_int);
py_wrap!(PyImport_ImportModule, (name: *const c_char) -> *mut PyObject);
py_wrap!(PyImport_GetModuleDict, () -> *mut PyObject);
py_wrap!(PyDict_SetItem, (dict: *mut PyObject, key: *const c_char, val: *mut PyObject) -> c_int);
py_wrap!(PyObject_GetAttrString, (obj: *mut PyObject, name: *const c_char) -> *mut PyObject);
py_wrap!(PyObject_SetAttrString, (obj: *mut PyObject, name: *const c_char, val: *mut PyObject) -> c_int);
py_wrap!(PyObject_CallObject, (callable: *mut PyObject, args: *mut PyObject) -> *mut PyObject);
py_wrap!(PyTuple_New, (len: Py_ssize_t) -> *mut PyObject);
py_wrap!(PyTuple_SetItem, (tuple: *mut PyObject, pos: Py_ssize_t, item: *mut PyObject) -> c_int);
py_wrap!(Py_IncRef, (obj: *mut PyObject) -> ());
py_wrap!(Py_DecRef, (obj: *mut PyObject) -> ());
py_wrap!(PyThreadState_New, (interp: *mut PyInterpreterState) -> *mut PyThreadState);
py_wrap!(PyEval_RestoreThread, (state: *mut PyThreadState) -> ());
py_wrap!(PyThreadState_Clear, (state: *mut PyThreadState) -> ());
py_wrap!(PyThreadState_DeleteCurrent, () -> ());
py_wrap!(PyEval_SaveThread, () -> *mut PyThreadState);
py_wrap!(PyUnicode_AsUTF8, (obj: *mut PyObject) -> *const c_char);
py_wrap!(PyUnicode_FromString, (obj: *const c_char) -> *mut PyObject);
py_wrap!(PyErr_Print, () -> ());
py_wrap!(PyCallable_Check, (callable: *mut PyObject) -> c_int);
py_wrap!(PyThreadState_Delete, (ts: *mut PyThreadState) -> ());
py_wrap!(PyObject_IsTrue, (obj: *mut PyObject) -> c_int);
py_wrap!(PyObject_Str, (obj: *mut PyObject) -> *mut PyObject);
py_wrap!(PySys_GetObject, (obj: *const c_char) -> *mut PyObject);
py_wrap!(PyList_Append, (list: *mut PyObject, item: *mut PyObject) -> c_int);
py_wrap!(PyModule_Create2, (module_def: *mut PyModuleDef, apiver: c_int) -> *mut PyObject);
py_wrap!(PyLong_FromUnsignedLongLong, (value: u64) -> *mut PyObject);
py_wrap!(PyLong_AsUnsignedLongLong, (obj: *mut PyObject) -> u64);
py_wrap!(PyTuple_GetItem, (tuple: *mut PyObject, pos: Py_ssize_t) -> *mut PyObject);
py_wrap!(PyErr_NewException, (name: *const c_char, base: *mut PyObject, dict: *mut PyObject) -> *mut PyObject);
py_wrap!(PyModule_AddObject, (module: *mut PyObject, name: *const c_char, value: *mut PyObject) -> c_int);
py_wrap!(PyErr_SetString, (type_: *mut PyObject, message: *const c_char) -> ());
py_wrap!(PyGILState_Ensure, () -> PyGILState_STATE);
py_wrap!(PyGILState_Release, (state: PyGILState_STATE) -> ());

#[unsafe(no_mangle)]
pub unsafe fn Py_XDECREF(obj: *mut PyObject) {
    if !obj.is_null() {
        Py_DecRef(obj);
    }
}

pub fn init_python() {
    INIT_PYTHON.call_once(|| {
        unsafe {
            load_python_library("/usr/lib/x86_64-linux-gnu/libpython3.11.so");
            Py_Initialize();
            let ts = PyThreadState_Get();
            let _ = MAIN_TS.set(ThreadStatePtr(ts));
            // Keep GIL released by default
            PyEval_SaveThread();
        }
    });
}

pub unsafe fn my_py_none_struct() -> *mut PyObject {
    let lib = get_python_lib();
    let symbol: Symbol<*mut PyObject> = lib.get(b"_Py_NoneStruct\0").unwrap();
    *symbol
}
