//
// Created by lewis on 10/9/22.
//

#include "../Websocket/WebsocketInterface.h"
#include "BundleDB.h"
#include "BundleManager.h"
#include <iostream>
#include <thread>

extern std::map<std::thread::id, std::string> threadBundleHashMap;
static PyObject *bundleDbError;

static auto create_or_update_job(PyObject * /*self*/, PyObject *args) -> PyObject *
{
    auto *dict = PyTuple_GetItem(args, 0);

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
        PyErr_SetString(bundleDbError, "Job was unable to be created or updated.");
        return nullptr;
    }

    // Set the job id in the job data
    jobId = response->pop_ulong();
    auto *value = PyLong_FromUnsignedLongLong(jobId);
    PyDict_SetItemString(dict, "job_id", value);

    Py_DECREF(value);
    Py_INCREF(dict);

    auto *result = PythonInterface::My_Py_NoneStruct();
    Py_INCREF(result);
    return result;
}

static auto get_job_by_id(PyObject * /*self*/, PyObject *args) -> PyObject *
{
    auto *jobIdObj = PyTuple_GetItem(args, 0);
    auto jobId = PyLong_AsUnsignedLongLong(jobIdObj);

    // Get the bundle hash
    auto bundleHash = threadBundleHashMap[std::this_thread::get_id()];
    auto bundleInterface = BundleManager::Singleton()->loadBundle(bundleHash);

    // Request the server save the record
    auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
    auto msg = Message(DB_BUNDLE_GET_JOB_BY_ID, Message::Priority::Medium, "database");
    msg.push_ulong(dbRequestId);
    msg.push_string(bundleHash);
    msg.push_ulong(jobId);

    msg.send();

    // Wait for the response
    auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

    // Check for success
    if (!response->pop_bool()) {
        PyErr_SetString(bundleDbError, ("Job with ID " + std::to_string(jobId) + " does not exist.").c_str());
        return nullptr;
    }

    // We can ignore the jobId
    response->pop_ulong();

    // Create a new dict from the result data
    auto *dict = bundleInterface->jsonLoads(response->pop_string());

    // Set the job id in the dict
    auto *value = PyLong_FromUnsignedLongLong(jobId);
    PyDict_SetItemString(dict, "job_id", value);

    Py_DECREF(value);
    Py_INCREF(dict);

    return dict;
}

static auto delete_job(PyObject * /*self*/, PyObject *args) -> PyObject *
{
    auto *dict = PyTuple_GetItem(args, 0);

    // Get the bundle hash
    auto bundleHash = threadBundleHashMap[std::this_thread::get_id()];
    auto bundleInterface = BundleManager::Singleton()->loadBundle(bundleHash);

    // Convert first argument to a json object
    auto jobData = nlohmann::json::parse(bundleInterface->jsonDumps(dict));

    // Request the server save the record
    auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
    auto msg = Message(DB_BUNDLE_DELETE_JOB, Message::Priority::Medium, "database");
    msg.push_ulong(dbRequestId);
    msg.push_string(bundleHash);

    // Try to get the job_id from the job data
    uint64_t jobId = 0;
    if (jobData.contains("job_id")) {
        jobId = jobData["job_id"];
    }

    if (jobId == 0) {
        PyErr_SetString(bundleDbError, "Job ID must be provided.");
        return nullptr;
    }

    msg.push_ulong(jobId);

    msg.send();

    // Wait for the response
    auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

    // Check for success
    if (!response->pop_bool()) {
        PyErr_SetString(bundleDbError, ("Job with ID " + std::to_string(jobId) + " does not exist.").c_str());
        return nullptr;
    }

    auto *result = PythonInterface::My_Py_NoneStruct();
    Py_INCREF(result);
    return result;
}

static std::array<PyMethodDef, 4> BundleDbMethods = {{
        {"create_or_update_job",  create_or_update_job, METH_VARARGS, "Updates a job record in the database if one already exists, otherwise inserts the job in to the database"},
        {"get_job_by_id",  get_job_by_id, METH_VARARGS, "Gets a job record if one exists for the provided id"},
        {"delete_job",  delete_job, METH_VARARGS, "Deletes a job record from the database"},
        {nullptr, nullptr, 0, nullptr}
}};

static struct PyModuleDef bundledbmodule = {
        PyModuleDef_HEAD_INIT,
        "_bundledb",
        nullptr,
        -1,
        BundleDbMethods.data()
};

PyMODINIT_FUNC PyInit_bundledb(void) // NOLINT(modernize-use-trailing-return-type)
{
    auto *pModule = PyModule_Create(&bundledbmodule);

    bundleDbError = PyErr_NewException("_bundledb.error", nullptr, nullptr);
    Py_XINCREF(bundleDbError);
    if (PyModule_AddObject(pModule, "error", bundleDbError) < 0) {
        Py_XDECREF(bundleDbError);
        Py_CLEAR(bundleDbError);
        Py_DECREF(pModule);
        return nullptr;
    }

    return pModule;
}