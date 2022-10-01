//
// Created by lewis on 9/3/22.
//

#ifndef ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
#define ADACS_JOB_CLIENT_BUNDLEINTERFACE_H


#include <string>
#include <Python.h>
#include <memory>
#include "PythonInterface.h"
#include "nlohmann/json.hpp"

class BundleInterface {
public:
    BundleInterface(const std::string& bundleHash);

    auto run(std::string bundleFunction, nlohmann::json details, std::string jobData) -> PyObject *;
    auto toString(PyObject*) -> std::string;
    uint64_t toUint64(PyObject *value);
    void disposeObject(PyObject*);

    void f(const char *tname);
private:
    std::shared_ptr<PythonInterface::SubInterpreter> pythonInterpreter;

    PyObject *pGlobal;
    PyObject *pLocal;
    PyObject *pBundleModule;

    PyObject *jsonModule;

    PyObject *jsonLoads(const std::string& content);
};


#endif //ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
