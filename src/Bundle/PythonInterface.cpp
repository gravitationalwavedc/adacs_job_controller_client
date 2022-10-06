//
// Created by lewis on 9/3/22.
//

#include <Python.h>
#include <dlfcn.h>
#include <iostream>

#include "PythonInterface.h"
#include "../lib/GeneralUtils.h"

static void* _dlPythonLibHandle = nullptr;

template<typename R, typename ... Args> R getRetType(R(*)(Args...));

#define ENSURE_DLFN(function, dlfn) if (dlfn == nullptr) { \
    std::cerr << "Unable to find required function " << #function << " in python dynamic library." << std::endl; \
    abortApplication();                                      \
}                                                          \
std::cout << #function << std::endl;

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
PYWRAP_1(PyModule_New, const char *, name)
PYWRAP_3(PyModule_AddStringConstant, PyObject *, a, const char *, b, const char *, c)
PYWRAP_5(Py_CompileStringExFlags, const char *, str, const char *, filename, int, start, PyCompilerFlags *, flags, int, optimize)
PYWRAP_1(PyModule_GetDict, PyObject *, o)
PYWRAP_0(PyErr_Occurred)
PYWRAP_0(PyErr_Print)
PYWRAP_3(PyEval_EvalCode, PyObject *, a, PyObject *, b, PyObject *, c)
PYWRAP_2(PyObject_GetAttrString, PyObject *, a, const char *, b)
PYWRAP_1(PyTuple_New, Py_ssize_t, size)
PYWRAP_3(PyTuple_SetItem, PyObject *, a, Py_ssize_t, b, PyObject *, c)
PYWRAP_2(PyObject_CallObject, PyObject *, callable, PyObject *, args)
PYWRAP_1(PyLong_AsLong, PyObject *, obj)
PYWRAP_0(Py_Finalize)
PYWRAP_1(PyLong_FromLong, long, value)
PYWRAP_0(PyEval_InitThreads)
PYWRAP_0(PyInterpreterState_New)
PYWRAP_1(PyThreadState_New, PyInterpreterState*, interp)
PYWRAP_0(PyThreadState_Get)
PYWRAP_1(PyThreadState_Swap, PyThreadState *, state)
PYWRAP_0(Py_NewInterpreter)
PYWRAP_1(Py_EndInterpreter, PyThreadState *, state)
PYWRAP_1(PyEval_RestoreThread, PyThreadState *, state)
PYWRAP_1(PyThreadState_Clear, PyThreadState *, state)
PYWRAP_0(PyThreadState_DeleteCurrent)
PYWRAP_0(PyEval_SaveThread)
//PYWRAP_1(PyObject_Repr, PyObject *, obj)
PYWRAP_1(PyUnicode_AsUTF8, PyObject *, obj)
PYWRAP_1(PyUnicode_FromString, const char *, obj)
PYWRAP_1(PyImport_ImportModule, const char *, obj)
PYWRAP_3(PyDict_Merge, PyObject*, mp, PyObject*, other, int, override)
PYWRAP_1(PySys_GetObject, const char *, obj)
PYWRAP_2(PyList_Append, PyObject *, list, PyObject *, item)

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
        std::cerr << "Unable to load python dynamic library " << sPythonLibrary << std::endl;
        abortApplication();
    }

    // Set the library handle and initialise the python interpreter
    _dlPythonLibHandle = libHandle;

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
    auto *my_Py_NoneStruct = reinterpret_cast<PyObject*>(dlsym(PythonInterface::getPythonLibHandle(), "_Py_NoneStruct"));

    return obj == my_Py_NoneStruct;
}
