//
// Created by lewis on 9/3/22.
//

#ifndef ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
#define ADACS_JOB_CLIENT_BUNDLEINTERFACE_H


#include <string>
#include <Python.h>
#include <memory>
#include "PythonInterface.h"

class BundleInterface {
public:
    BundleInterface(std::string bundlePath);

    void f(const char *tname);
private:
    std::shared_ptr<PythonInterface::SubInterpreter> pythonInterpreter;

    PyObject *pGlobal;
    PyObject *pLocal;
    PyObject *pBundleModule;
};


#endif //ADACS_JOB_CLIENT_BUNDLEINTERFACE_H
