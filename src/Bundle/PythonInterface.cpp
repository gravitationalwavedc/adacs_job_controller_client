//
// Created by lewis on 9/3/22.
//

#include <Python.h>
#include <dlfcn.h>
#include <iostream>
#include <glog/logging.h>

#include "PythonInterface.h"
#include "../lib/GeneralUtils.h"
#include "BundleDB.h"

static void* _dlPythonLibHandle = nullptr;

template<typename R, typename ... Args> R getRetType(R(*)(Args...));

#define ENSURE_DLFN(function, dlfn) if (dlfn == nullptr) { \
    LOG(ERROR) << "Unable to find required function " << #function << " in python dynamic library."; \
    abortApplication();                                      \
}

#define PYWRAP_0(function) auto function() -> decltype(function()) { \
 using ReturnType = decltype( getRetType(&function ) );                       \
 auto func = reinterpret_cast<ReturnType (*)(...)>(dlsym(PythonInterface::getPythonLibHandle(), #function)); \
 ENSURE_DLFN(function, func) \
 return func(); \
};

#define PYWRAP_1(function, t1, a1) auto function(t1 a1) -> decltype(getRetType(&function)) { \
 using ReturnType = decltype( getRetType(&function ) );                       \
 auto func = reinterpret_cast<ReturnType (*)(...)>(dlsym(PythonInterface::getPythonLibHandle(), #function)); \
 ENSURE_DLFN(function, func) \
 return func(a1); \
};

#define PYWRAP_2(function, t1, a1, t2, a2) auto function(t1 a1, t2 a2) -> decltype(getRetType(&function)) { \
 using ReturnType = decltype( getRetType(&function ) );                       \
 auto func = reinterpret_cast<ReturnType (*)(...)>(dlsym(PythonInterface::getPythonLibHandle(), #function));\
 ENSURE_DLFN(function, func) \
 return func(a1, a2); \
};

#define PYWRAP_3(function, t1, a1, t2, a2, t3, a3) auto function(t1 a1, t2 a2, t3 a3) -> decltype(getRetType(&function)) { \
 using ReturnType = decltype( getRetType(&function ) );                       \
 auto func = reinterpret_cast<ReturnType (*)(...)>(dlsym(PythonInterface::getPythonLibHandle(), #function));               \
 ENSURE_DLFN(function, func) \
 return func(a1, a2, a3); \
};

#define PYWRAP_5(function, t1, a1, t2, a2, t3, a3, t4, a4, t5, a5) auto function(t1 a1, t2 a2, t3 a3, t4 a4, t5 a5) -> decltype(getRetType(&function)) { \
 using ReturnType = decltype( getRetType(&function ) );                       \
 auto func = reinterpret_cast<ReturnType (*)(...)>(dlsym(PythonInterface::getPythonLibHandle(), #function));               \
 ENSURE_DLFN(function, func) \
 return func(a1, a2, a3, a4, a5); \
};

extern "C" {
// NOLINTBEGIN(cppcoreguidelines-pro-type-reinterpret-cast)
PYWRAP_0(Py_Initialize)
PYWRAP_0(PyDict_New)
PYWRAP_0(PyEval_GetBuiltins)
PYWRAP_3(PyDict_SetItemString, PyObject *, dp, const char *, key, PyObject *, item)
PYWRAP_1(PyModule_GetDict, PyObject *, o)
PYWRAP_0(PyErr_Occurred)
PYWRAP_0(PyErr_Print)
PYWRAP_2(PyObject_GetAttrString, PyObject *, a, const char *, b)
PYWRAP_1(PyTuple_New, Py_ssize_t, size)
PYWRAP_3(PyTuple_SetItem, PyObject *, a, Py_ssize_t, b, PyObject *, c)
PYWRAP_2(PyObject_CallObject, PyObject *, callable, PyObject *, args)
PYWRAP_1(PyLong_AsLong, PyObject *, obj)
PYWRAP_0(PyEval_InitThreads)
PYWRAP_1(PyThreadState_New, PyInterpreterState*, interp)
PYWRAP_0(PyThreadState_Get)
PYWRAP_1(PyThreadState_Swap, PyThreadState *, state)
PYWRAP_0(Py_NewInterpreter)
PYWRAP_1(Py_EndInterpreter, PyThreadState *, state)
PYWRAP_1(PyEval_RestoreThread, PyThreadState *, state)
PYWRAP_1(PyThreadState_Clear, PyThreadState *, state)
PYWRAP_0(PyThreadState_DeleteCurrent)
PYWRAP_0(PyEval_SaveThread)
PYWRAP_1(PyUnicode_AsUTF8, PyObject *, obj)
PYWRAP_1(PyUnicode_FromString, const char *, obj)
PYWRAP_1(PyImport_ImportModule, const char *, obj)
PYWRAP_1(PySys_GetObject, const char *, obj)
PYWRAP_2(PyList_Append, PyObject *, list, PyObject *, item)
PYWRAP_2(PyModule_Create2, struct PyModuleDef*, moduleDef, int, apiver)
PYWRAP_1(PyObject_Repr, PyObject *, object)
PYWRAP_1(PyLong_FromLong, long, value)
PYWRAP_3(PyDict_SetItem, PyObject*, dict, PyObject*, key, PyObject*, value)
PYWRAP_1(PyObject_Type, PyObject*, object)
PYWRAP_2(PyTuple_GetItem, PyObject*, tuple, Py_ssize_t, pos)

// Exceptional functions (weird arguments or whatever)
auto PyImport_AppendInittab(const char * name, PyObject* (*initfunc)(void)) -> decltype(getRetType(&PyImport_AppendInittab)) { \
 using ReturnType = decltype( getRetType(&PyImport_AppendInittab ) );                       \
 auto func = reinterpret_cast<ReturnType (*)(...)>(dlsym(PythonInterface::getPythonLibHandle(), "PyImport_AppendInittab"));\
 ENSURE_DLFN(PyImport_AppendInittab, func) \
 return func(name, initfunc); \
};

// Python 3.8+ changed this to a function
#if PY_MINOR_VERSION >= 8
PYWRAP_1(_Py_Dealloc, PyObject *, obj)
#endif
// NOLINTEND(cppcoreguidelines-pro-type-reinterpret-cast)
}

void PythonInterface::initPython(const std::string& sPythonLibrary) {
    // Attempt to load the python dynamic library
    void *libHandle = dlopen(sPythonLibrary.c_str(), RTLD_NOW | RTLD_GLOBAL);
    if (libHandle == nullptr) {
        LOG(ERROR) << "Unable to load python dynamic library " << sPythonLibrary;
        abortApplication();
    }

    // Set the library handle and initialise the python interpreter
    _dlPythonLibHandle = libHandle;

    // Expose the _bundledb module, should be before Py_Initialize()
    if (PyImport_AppendInittab("_bundledb", PyInit_bundledb) == -1) {
        LOG(ERROR) << "Could not extend in-built modules table";
        abortApplication();
    }

    Py_Initialize();
    PyEval_InitThreads();
}

auto PythonInterface::getPythonLibHandle() -> void * {
    assert(_dlPythonLibHandle);

    return _dlPythonLibHandle;
}

auto PythonInterface::newInterpreter() -> std::shared_ptr<SubInterpreter> {
    return std::make_shared<SubInterpreter>();
}

auto PythonInterface::MyPy_IsNone(PyObject *obj) -> bool {
    static auto *my_Py_NoneStruct = reinterpret_cast<PyObject*>(dlsym(PythonInterface::getPythonLibHandle(), "_Py_NoneStruct"));

    return obj == my_Py_NoneStruct;
}

auto PythonInterface::My_Py_NoneStruct() -> PyObject * {
    static auto *my_Py_NoneStruct = reinterpret_cast<PyObject*>(dlsym(PythonInterface::getPythonLibHandle(), "_Py_NoneStruct"));

    return my_Py_NoneStruct;
}
