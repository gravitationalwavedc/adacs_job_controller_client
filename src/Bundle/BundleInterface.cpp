//
// Created by lewis on 9/3/22.
//

#include "../Lib/GeneralUtils.h"
#include "BundleInterface.h"
#include <glog/logging.h>
#include <iostream>
#include <thread>

std::map<std::thread::id, std::string> threadBundleHashMap;

const char* stdoutRedirection = R"PY(
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
)PY";

BundleInterface::BundleInterface(const std::string& bundleHash) : bundleHash(bundleHash) {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> const lock(mutex_);

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
    PythonInterface::SubInterpreter::ThreadScope const scope(pythonInterpreter->interp());

    auto bundlePath = boost::filesystem::path(getBundlePath()) / bundleHash;

    // Create a new globals dict and enable the python builtins
    pGlobal = PyDict_New();
    PyDict_SetItemString(pGlobal, "__builtins__", PyEval_GetBuiltins());

    // Set up the logging so print() works as expected
    auto *pLocal = PyDict_New();
    PyUnicode_AsUTF8(PyObject_Repr(PyRun_String(stdoutRedirection, Py_file_input, pGlobal, pLocal)));

    // Ensure the json module is loaded in the global scope
    jsonModule = PyImport_ImportModule("json");
    PyDict_SetItemString(pGlobal, "json", jsonModule);

    // Load the traceback module
    tracebackModule = PyImport_ImportModule("traceback");

    // Add the bundle path to the system path and then import the bundle
    auto *pPath = PySys_GetObject("path");
    PyList_Append(pPath, PyUnicode_FromString(bundlePath.c_str()));

    // Create a new python module
    pBundleModule = PyImport_ImportModule("bundle");
    if (PyErr_Occurred() != nullptr) {
        LOG(ERROR) << "Error loading python bundle at path " << bundlePath;
        printLastPythonException();
        abortApplication();
    }
}

void BundleInterface::printLastPythonException() {
    // Get the exception
    PyObject* extype = nullptr;
    PyObject* value = nullptr;
    PyObject* traceback = nullptr;

    PyErr_Fetch(&extype, &value, &traceback);
    if (!extype) {
        LOG(INFO) << "No active python exception to print";
        return;
    }

    // Get a pointer to the format_exception function
    auto* pFunc = PyObject_GetAttrString(tracebackModule, "format_exception");

    // Build a tuple to hold the arguments
    auto* pArgs = PyTuple_New(3);
    PyTuple_SetItem(pArgs, 0, extype);
    PyTuple_SetItem(pArgs, 1, value);
    PyTuple_SetItem(pArgs, 2, traceback);

    // Call the function, passing it the exception args
    PyObject* pLines = PyObject_CallObject(pFunc, pArgs);
    if (PyErr_Occurred() != nullptr) {
        throw std::runtime_error("Error printing active python exception");
    }

    // Now iterate over the lines returned in the exception and print them
    auto iter =  PyObject_GetIter(pLines);

    if (iter != nullptr) {
        auto item = PyIter_Next(iter);

        while (item != nullptr) {
            const char *unicode_item = PyUnicode_AsUTF8(item);
            LOG(INFO) << unicode_item;

            Py_DECREF(item);

            item = PyIter_Next(iter);
        }

        Py_DECREF(iter);
    }

    if (extype != nullptr)
        Py_DECREF(extype);

    if (value != nullptr)
        Py_DECREF(value);

    if (traceback != nullptr)
        Py_DECREF(traceback);

    Py_DECREF(pArgs);
    Py_XDECREF(pFunc);
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
        printLastPythonException();
        throw std::runtime_error("Error calling json.loads");
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
        printLastPythonException();
        throw std::runtime_error("Error calling bundle function " + bundleFunction);
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

auto BundleInterface::toString(PyObject *object) -> std::string {
    return {PyUnicode_AsUTF8(object)};
}

auto BundleInterface::toUint64(PyObject *value) -> uint64_t {
    return PyLong_AsUnsignedLongLong(value);
}

auto BundleInterface::toBool(PyObject *value) -> bool {
    return value == PythonInterface::My_Py_TrueStruct();
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
        printLastPythonException();
        throw std::runtime_error("Error calling json.dumps");
    }

    Py_DECREF(pArgs);
    Py_XDECREF(pFunc);

    return {PyUnicode_AsUTF8(pValue)};
}

void BundleInterface::disposeObject(PyObject* object) {
    Py_DECREF(object);
}
