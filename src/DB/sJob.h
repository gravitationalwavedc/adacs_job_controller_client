//
// Created by lewis on 10/5/22.
//

#ifndef ADACS_JOB_CLIENT_SJOB_H
#define ADACS_JOB_CLIENT_SJOB_H

#include "../Lib/Messaging/Message.h"
#include "../Websocket/WebsocketInterface.h"
#include <cstdint>
#include <string>
#include <thread>


struct sJob {
    static auto fromMessage(const std::shared_ptr<Message>& msg) -> sJob {
        return {
                .id = msg->pop_ulong(),
                .jobId = msg->pop_ulong(),
                .schedulerId = msg->pop_ulong(),
                .submitting = msg->pop_bool(),
                .submittingCount = msg->pop_uint(),
                .bundleHash = msg->pop_string(),
                .workingDirectory = msg->pop_string(),
                .queued = msg->pop_bool(),
                .params = msg->pop_string(),
                .running = msg->pop_bool()
        };
    }

    static auto getOrCreateByJobId(uint64_t jobId) -> sJob {
        // Request the data from the server
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOB_GET_BY_JOB_ID, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(jobId);
        msg.send();

        // Wait for the response
        auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

        // Check for success
        if (!response->pop_bool()) {
            throw std::runtime_error("Unable to get record");
        }

        // Check that a record was provided
        if (response->pop_uint() == 0) {
            // No, return a new object
            return {};
        }

        // Return a job object from the message
        return sJob::fromMessage(response);
    }

    static auto getRunningJobs() -> std::vector<sJob> {
        // Request the running jobs from the server
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOB_GET_RUNNING_JOBS, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.send();

        // Wait for the response
        auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

        // Check for success
        if (!response->pop_bool()) {
            throw std::runtime_error("Unable to get record");
        }

        // Get the number of returned records
        auto numRecords = response->pop_uint();

        // Read in the records as job objects
        std::vector<sJob> jobs;
        for (uint32_t index = 0; index < numRecords; index++) {
            jobs.push_back(fromMessage(response));
        }

        return jobs;
    }

    void _delete() const {
        // Request the server delete this record
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOB_DELETE, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(id);
        msg.send();

        // Wait for the response
        auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

        // Check for success
        if (!response->pop_bool()) {
            throw std::runtime_error("Unable to delete record");
        }
    }

    void save() {
        // Request the server save the record
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOB_SAVE, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(id);
        msg.push_ulong(jobId);
        msg.push_ulong(schedulerId);
        msg.push_bool(submitting);
        msg.push_uint(submittingCount);
        msg.push_string(bundleHash);
        msg.push_string(workingDirectory);
        msg.push_bool(queued);
        msg.push_string(params);
        msg.push_bool(running);
        msg.send();

        // Wait for the response
        auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

        // Check for success
        if (!response->pop_bool()) {
            throw std::runtime_error("Unable to save record");
        }

        // Set the job id
        id = response->pop_ulong();
    }

    static auto getJobById(uint64_t pkid) -> sJob {
        // Request the data from the server
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOB_GET_BY_ID, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(pkid);
        msg.send();

        // Wait for the response
        auto response = WebsocketInterface::Singleton()->getDbResponse(dbRequestId);

        // Check for success
        if (!response->pop_bool()) {
            throw std::runtime_error("Unable to get record");
        }

        // Check that a record was provided
        if (response->pop_uint() == 0) {
            // Nope, record didn't exist
            throw std::runtime_error("Unable to get record");
        }

        // Return a job object from the message
        return sJob::fromMessage(response);
    }

    void refreshFromDb() {
        if (id == 0) {
            throw std::runtime_error("Can't refresh a record without an id");
        }

        *this = getJobById(id);
    }

    uint64_t id = 0;
    uint64_t jobId = 0;
    uint64_t schedulerId = 0;
    bool submitting = false;
    uint32_t submittingCount = 0;
    std::string bundleHash;
    std::string workingDirectory;
    bool queued = false;
    std::string params;
    bool running = false;
};


#endif //ADACS_JOB_CLIENT_SJOB_H
