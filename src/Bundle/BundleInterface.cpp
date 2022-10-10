//
// Created by lewis on 9/3/22.
//

#include "../lib/GeneralUtils.h"
#include "BundleInterface.h"
#include <glog/logging.h>
#include <iostream>
#include <thread>

std::map<std::thread::id, std::string> threadBundleHashMap;

BundleInterface::BundleInterface(const std::string& bundleHash) : bundleHash(bundleHash) {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> lock(mutex_);

    static PyThreadState *_state = nullptr;
    if (_state != nullptr) {
        PyEval_RestoreThread(_state);
        _state = nullptr;
    }

    pythonInterpreter = PythonInterface::newInterpreter();

    if (_state == nullptr) {
        _state = PyEval_SaveThread();
    }

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
        LOG(ERROR) << "Error loading python bundle at path " << bundlePath;
        PyErr_Print();
        abortApplication();
    }

    // Get the locals dict
    pLocal = PyModule_GetDict(pBundleModule);
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

auto BundleInterface::run(const std::string& bundleFunction, const nlohmann::json& details, const std::string& jobData) -> PyObject* {
    // First we need to create a python object from the details json
    auto *jsonObj = jsonLoads(details.dump());

    // Get a pointer to the bundle function to call
    auto* pFunc = PyObject_GetAttrString(pBundleModule, bundleFunction.c_str());

    // Build a tuple to hold the arguments
    auto* pArgs = PyTuple_New(2);
    PyTuple_SetItem(pArgs, 0, jsonObj);
    auto* pValue = PyUnicode_FromString(jobData.c_str());
    PyTuple_SetItem(pArgs, 1, pValue);

    // Set up the thread bundle hash map
    threadBundleHashMap.emplace(std::this_thread::get_id(), bundleHash);

    // Call the bundle function
    auto *pResult = PyObject_CallObject(pFunc, pArgs);
    if (PyErr_Occurred() != nullptr) {
        LOG(ERROR) << "Error calling bundle function " << bundleFunction;
        PyErr_Print();
        abortApplication();
    }

    // Clear the thread from the thread bundle hash map
    threadBundleHashMap.erase(std::this_thread::get_id());

    Py_DECREF(pArgs);
    Py_XDECREF(pFunc);

    if (PythonInterface::MyPy_IsNone(pResult)) {
        throw none_exception();
    }

    return pResult;
}

auto BundleInterface::toString(PyObject *value) -> std::string {
    return {PyUnicode_AsUTF8(value)};
}

auto BundleInterface::toUint64(PyObject *value) -> uint64_t {
    return PyLong_AsUnsignedLongLong(value);
}

auto BundleInterface::jsonDumps(PyObject* obj) -> std::string {
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
    Py_DECREF(object);
}
