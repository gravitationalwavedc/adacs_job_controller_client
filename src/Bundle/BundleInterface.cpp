//
// Created by lewis on 9/3/22.
//

#include "BundleInterface.h"

BundleInterface::BundleInterface(std::string bundlePath) {
    pythonInterpreter = PythonInterface::newInterpreter();

    auto threadScope = PythonInterface::enableThreading();

    // Activate the new interpreter
    PythonInterface::SubInterpreter::ThreadScope scope(pythonInterpreter->interp());

    // Create a new globals dict and enable the python builtins
    pGlobal = PyDict_New();
    PyDict_SetItemString(pGlobal, "__builtins__", PyEval_GetBuiltins());

    // Create a new python module
    pBundleModule = PyModule_New("bundle");

    // Get the locals dict
    pLocal = PyModule_GetDict(pBundleModule);

    auto *pValue = Py_CompileString("def blah(x):\n\tprint(5 * x)\n\timport sys\n\tprint(\"Python version\")\n\tprint (sys.version)\n\tprint(\"Version info.\")\n\tprint (sys.version_info)\n\treturn 77\n", "source", Py_file_input);
    if (PyErr_Occurred() != nullptr) {
        PyErr_Print();
        std::terminate();
    }

    pValue = PyEval_EvalCode(pValue, pGlobal, pLocal);
    Py_DECREF(pValue);

    //Get a pointer to the init
    auto* pFunc = PyObject_GetAttrString(pBundleModule, "blah");

    //Build a tuple to hold my arguments (just the number 4 in this case)
    auto* pArgs = PyTuple_New(1);
    pValue = PyLong_FromLong(4);
    PyTuple_SetItem(pArgs, 0, pValue);

    //Call my function, passing it the number four
    pValue = PyObject_CallObject(pFunc, pArgs);
    if (PyErr_Occurred() != nullptr) {
        PyErr_Print();
        std::terminate();
    }

    Py_DECREF(pArgs);
    printf("Returned val: %ld\n", PyLong_AsLong(pValue));
    Py_DECREF(pValue);

    Py_XDECREF(pFunc);
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
        std::terminate();
    }

    pValue = PyEval_EvalCode(pValue, pGlobal, pLocal);
    Py_DECREF(pValue);
}