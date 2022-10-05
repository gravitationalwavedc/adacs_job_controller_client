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

    auto run(const std::string& bundleFunction, nlohmann::json details, std::string jobData) -> PyObject *;
    auto toString(PyObject*) -> std::string;
    auto toUint64(PyObject *value) -> uint64_t;
    auto jsonDumps(PyObject *obj) -> std::string;
    void disposeObject(PyObject*);

    void f(const char *tname);

    class none_exception : public std::exception {

    };
private:
    std::shared_ptr<PythonInterface::SubInterpreter> pythonInterpreter;

    PyObject *pGlobal;
    PyObject *pLocal;
    PyObject *pBundleModule;

    PyObject *jsonModule;

    auto jsonLoads(const std::string& content) -> PyObject *;
};


#endif //ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
