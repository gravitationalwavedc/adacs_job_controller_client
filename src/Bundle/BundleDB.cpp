//
// Created by lewis on 10/9/22.
//

#include <iostream>
#include "BundleDB.h"
#include "BundleManager.h"
#include "../Websocket/WebsocketInterface.h"
#include "../lib/Messaging/Message.h"
#include <thread>

extern std::map<std::thread::id, std::string> threadBundleHashMap;

static PyObject * create_or_update_job(PyObject *self, PyObject *args)
{
    auto dict = PyTuple_GetItem(args, 0);

    // Get the bundle hash
    auto bundleHash = threadBundleHashMap[std::this_thread::get_id()];
    auto bundleInterface = BundleManager::Singleton()->loadBundle(bundleHash);

    // Convert first argument to a json object
    auto jobData = nlohmann::json::parse(bundleInterface->jsonDumps(dict));

    // Request the server save the record
    auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
    auto msg = Message(DB_BUNDLE_CREATE_OR_UPDATE_JOB, Message::Priority::Medium, "database");
    msg.push_ulong(dbRequestId);
    msg.push_string(bundleHash);

    // Try to get the job_id from the job data
    uint64_t jobId = 0;
    if (jobData.contains("job_id")) {
        jobId = jobData["job_id"];

        // Remove the job id from the job data
        jobData.erase("job_id");
    }

    msg.push_ulong(jobId);
    msg.push_string(jobData.dump());

    msg.send();

    // Wait for the response
    auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

    // Check for success
    if (!response->pop_bool()) {
        throw std::runtime_error("Unable to save record");
    }

    // Set the job id in the job data
    jobId = response->pop_ulong();
    auto value = PyLong_FromLong(jobId);
    PyDict_SetItemString(dict, "job_id", value);

    Py_DECREF(value);
    Py_INCREF(dict);

    return Py_NewRef(PythonInterface::My_Py_NoneStruct());
}

static PyMethodDef BundleDbMethods[] = {
        {"create_or_update_job",  create_or_update_job, METH_VARARGS, "Updates a job record in the database if one already exists, otherwise inserts the job in to the database"},
        {NULL, NULL, 0, NULL}        /* Sentinel */
};

static struct PyModuleDef bundledbmodule = {
        PyModuleDef_HEAD_INIT,
        "_bundledb",    /* name of module */
        NULL,           /* module documentation, may be NULL */
        -1,             /* size of per-interpreter state of the module,
                        or -1 if the module keeps state in global variables. */
        BundleDbMethods
};

PyMODINIT_FUNC PyInit_bundledb(void)
{
    return PyModule_Create(&bundledbmodule);
}