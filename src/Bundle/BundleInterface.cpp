//
// Created by lewis on 9/3/22.
//

#include <iostream>
#include "BundleInterface.h"
#include "../lib/GeneralUtils.h"

BundleInterface::BundleInterface(const std::string& bundleHash) {
    pythonInterpreter = PythonInterface::newInterpreter();

    auto threadScope = PythonInterface::enableThreading();

    // Activate the new interpreter
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    auto bundlePath = boost::filesystem::path(getBundlePath()) / bundleHash;

    // Create a new globals dict and enable the python builtins
    pGlobal = PyDict_New();
    PyDict_SetItemString(pGlobal, "__builtins__", PyEval_GetBuiltins());

    // Ensure the json module is loaded in the global scope
    jsonModule = PyImport_ImportModule("json");
    PyDict_SetItemString(pGlobal, "json", jsonModule);

    // Add the bundle path to the system path and then import the bundle
    auto *pPath = PySys_GetObject("path");
    PyList_Append(pPath, PyUnicode_FromString(bundlePath.c_str()));

    // Create a new python module
    pBundleModule = PyImport_ImportModule("bundle");
    if (PyErr_Occurred() != nullptr) {
        std::cerr << "Error loading python bundle at path " << bundlePath << std::endl;
        PyErr_Print();
        abortApplication();
    }

    // Get the locals dict
    pLocal = PyModule_GetDict(pBundleModule);

//    auto pValue = Py_CompileString("def blah(x):\n\tprint(5 * x)\n\timport sys\n\tprint(\"Python version\")\n\tprint (sys.version)\n\tprint(\"Version info.\")\n\tprint (sys.version_info)\n\treturn 77\n", "source", Py_file_input);
//    if (PyErr_Occurred() != nullptr) {
//        PyErr_Print();
//        abortApplication();
//    }
//
//    pValue = PyEval_EvalCode(pValue, pGlobal, pLocal);
//    Py_DECREF(pValue);
//
//    //Get a pointer to the init
//    auto* pFunc = PyObject_GetAttrString(pBundleModule, "blah");
//
//    //Build a tuple to hold my arguments (just the number 4 in this case)
//    auto* pArgs = PyTuple_New(1);
//    pValue = PyLong_FromLong(4);
//    PyTuple_SetItem(pArgs, 0, pValue);
//
//    //Call my function, passing it the number four
//    pValue = PyObject_CallObject(pFunc, pArgs);
//    if (PyErr_Occurred() != nullptr) {
//        PyErr_Print();
//        abortApplication();
//    }
//
//    Py_DECREF(pArgs);
//    printf("Returned val: %ld\n", PyLong_AsLong(pValue));
//    Py_DECREF(pValue);
//
//    Py_XDECREF(pFunc);
}

void BundleInterface::f(const char* tname)
{
    std::string code = R"PY(
from __future__ import print_function
import sys
print("TNAME: sys.xxx={}".format(getattr(sys, 'xxx', 'attribute not set')))
    )PY";

    code.replace(code.find("TNAME"), 5, tname);

    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    auto *pValue = Py_CompileString(code.c_str(), "source", Py_file_input);
    if (PyErr_Occurred() != nullptr) {
        PyErr_Print();
        abortApplication();
    }

    pValue = PyEval_EvalCode(pValue, pGlobal, pLocal);
    Py_DECREF(pValue);
}

auto BundleInterface::jsonLoads(const std::string& content) -> PyObject* {
    // Get a pointer to the json.loads function
    auto* pFunc = PyObject_GetAttrString(jsonModule, "loads");

    // Build a tuple to hold the arguments
    auto* pArgs = PyTuple_New(1);
    auto* pValue = PyUnicode_FromString(content.c_str());
    PyTuple_SetItem(pArgs, 0, pValue);

    //Call my function, passing it the number four
    pValue = PyObject_CallObject(pFunc, pArgs);
    if (PyErr_Occurred() != nullptr) {
        PyErr_Print();
        abortApplication();
    }

    Py_DECREF(pArgs);
    Py_XDECREF(pFunc);

    return pValue;
}

auto BundleInterface::run(const std::string& bundleFunction, nlohmann::json details, std::string jobData) -> PyObject* {
    auto threadScope = PythonInterface::enableThreading();
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    // First we need to create a python object from the details json
    auto *jsonObj = jsonLoads(details.dump());

    // Get a pointer to the bundle function to call
    auto* pFunc = PyObject_GetAttrString(pBundleModule, bundleFunction.c_str());

    // Build a tuple to hold the arguments
    auto* pArgs = PyTuple_New(2);
    PyTuple_SetItem(pArgs, 0, jsonObj);
    auto* pValue = PyUnicode_FromString(jobData.c_str());
    PyTuple_SetItem(pArgs, 1, pValue);

    // Call the bundle function
    auto *pResult = PyObject_CallObject(pFunc, pArgs);
    if (PyErr_Occurred() != nullptr) {
        std::cerr << "Error calling bundle function " << bundleFunction << std::endl;
        PyErr_Print();
        abortApplication();
    }

    Py_DECREF(pArgs);
    Py_XDECREF(pFunc);

    if (PythonInterface::MyPy_IsNone(pResult)) {
        throw none_exception();
    }

    return pResult;
}

auto BundleInterface::toString(PyObject *value) -> std::string {
    auto threadScope = PythonInterface::enableThreading();
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    return std::string{PyUnicode_AsUTF8(value)};
}

auto BundleInterface::toUint64(PyObject *value) -> uint64_t {
    auto threadScope = PythonInterface::enableThreading();
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    return PyLong_AsLong(value);
}

auto BundleInterface::jsonDumps(PyObject* obj) -> std::string {
    auto threadScope = PythonInterface::enableThreading();
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    // Get a pointer to the json.loads function
    auto* pFunc = PyObject_GetAttrString(jsonModule, "dumps");

    // Build a tuple to hold the arguments
    auto* pArgs = PyTuple_New(1);
    PyTuple_SetItem(pArgs, 0, obj);

    // Call json.dumps on the provided object
    auto *pValue = PyObject_CallObject(pFunc, pArgs);
    if (PyErr_Occurred() != nullptr) {
        PyErr_Print();
        abortApplication();
    }

    Py_DECREF(pArgs);
    Py_XDECREF(pFunc);

    return {PyUnicode_AsUTF8(pValue)};
}

void BundleInterface::disposeObject(PyObject* object) {
    auto threadScope = PythonInterface::enableThreading();
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    Py_DECREF(object);
}
