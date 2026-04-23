//
// Created by lewis on 9/3/22.
//

#ifndef ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
#define ADACS_JOB_CLIENT_BUNDLEINTERFACE_H


#include "PythonInterface.h"
#include "nlohmann/json.hpp"
#include <Python.h>
#include <memory>
#include <string>

class BundleInterface {
public:
    explicit BundleInterface(const std::string& bundleHash);

    auto run(const std::string& bundleFunction, const nlohmann::json& details, const std::string& jobData) -> PyObject *;
    static auto toString(PyObject* object) -> std::string;
    static auto toUint64(PyObject *value) -> uint64_t;
    static auto toBool(PyObject *value) -> bool;
    auto jsonDumps(PyObject *obj) -> std::string;
    auto jsonLoads(const std::string& content) -> PyObject *;
    static void disposeObject(PyObject* object);
    void printLastPythonException();

    auto threadScope() -> PythonInterface::SubInterpreter::ThreadScope {
        return PythonInterface::SubInterpreter::ThreadScope {pythonInterpreter->interp()};
    }

    class none_exception : public std::exception {

    };
private:
    std::shared_ptr<PythonInterface::SubInterpreter> pythonInterpreter;

    PyObject *pGlobal;
    PyObject *pBundleModule;

    PyObject *jsonModule;
    PyObject *tracebackModule;

    std::string bundleHash;
};


#endif //ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
