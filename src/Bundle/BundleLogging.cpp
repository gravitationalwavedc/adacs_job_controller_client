//
// Created by lewis on 10/9/22.
//

#include "../Websocket/WebsocketInterface.h"
#include "BundleLogging.h"
#include "BundleManager.h"
#include <iostream>
#include <thread>

extern std::map<std::thread::id, std::string> threadBundleHashMap;
std::vector<std::string> lineParts;

static auto writeLog(PyObject * /*self*/, PyObject *args) -> PyObject * {
    // Get the bundle hash
    auto bundleHash = threadBundleHashMap[std::this_thread::get_id()];

    // Convert first argument to a bool
    auto *arg = PyTuple_GetItem(args, 0);

    // Note that we don't use the BundleInterface here - because it can be called recursively while loading the bundle
    auto bStdOut = arg == PythonInterface::My_Py_TrueStruct();;

    // Convert the second argument to a string
    arg = PyTuple_GetItem(args, 1);

    // Note that we don't use the BundleInterface here - because it can be called recursively while loading the bundle
    std::string message = {PyUnicode_AsUTF8(arg)};

    // Don't write trailing newlines
    if (message != "\n") {
        lineParts.push_back(message);
    } else {
#ifdef BUILD_TESTS
        extern std::string lastBundleLoggingMessage;
        extern bool lastBundleLoggingbStdOut;
        lastBundleLoggingbStdOut = bStdOut;
#endif

        message = "Bundle [" + bundleHash + "]: ";

        for (auto &bit: lineParts) {
            message += bit;
        }

        lineParts.clear();

#ifdef BUILD_TESTS
        lastBundleLoggingMessage = message;
#endif

        if (bStdOut) {
            LOG(INFO) << message;
        } else {
            LOG(ERROR) << message;
        }
    }

    auto *result = PythonInterface::My_Py_NoneStruct();
    Py_INCREF(result);
    return result;
}

static std::array<PyMethodDef, 2> BundleLoggingMethods = {
    {
              {"write", writeLog, METH_VARARGS,
               "Writes the provided message to the client log file."},
              {nullptr, nullptr, 0, nullptr}
    }
};

static struct PyModuleDef bundleloggingmodule = {
        PyModuleDef_HEAD_INIT,
        "_bundlelogging",
        nullptr,
        -1,
        BundleLoggingMethods.data()
};

PyMODINIT_FUNC PyInit_bundlelogging(void) // NOLINT(modernize-use-trailing-return-type)
{
    auto *pModule = PyModule_Create(&bundleloggingmodule);

    return pModule;
}