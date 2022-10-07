//
// Created by lewis on 10/5/22.
//

#ifndef ADACS_JOB_CLIENT_SSTATUS_H
#define ADACS_JOB_CLIENT_SSTATUS_H

#include <cstdint>
#include <string>
#include <thread>


struct sStatus {
    uint64_t id;
    uint64_t jobId;
    std::string what;
    uint32_t state;

    static auto fromMessage(const std::shared_ptr<Message>& msg) -> sStatus {
        return {
                .id = msg->pop_ulong(),
                .jobId = msg->pop_ulong(),
                .what = msg->pop_string(),
                .state = msg->pop_uint()
        };
    }

    static auto getJobStatusByJobIdAndWhat(uint64_t jobId, const std::string& what) {
        // Request the data from the server
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(jobId);
        msg.push_string(what);
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
        std::vector<sStatus> vStatus;
        for (uint32_t index = 0; index < numRecords; index++) {
            vStatus.push_back(fromMessage(response));
        }

        return vStatus;
    }

    static auto getJobStatusByJobId(uint64_t jobId) {
        // Request the data from the server
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOBSTATUS_GET_BY_JOB_ID, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(jobId);
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
        std::vector<sStatus> vStatus;
        for (uint32_t index = 0; index < numRecords; index++) {
            vStatus.push_back(fromMessage(response));
        }

        return vStatus;
    }

    static void deleteByIdList(const std::vector<uint64_t>& ids) {
        // Request the server delete records
        auto dbRequestId = WebsocketInterface::Singleton()->generateDbRequestId();
        auto msg = Message(DB_JOBSTATUS_DELETE_BY_ID_LIST, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_uint(ids.size());
        for (const auto& value : ids) {
            msg.push_ulong(value);
        }
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
        auto msg = Message(DB_JOBSTATUS_SAVE, Message::Priority::Medium, "database");
        msg.push_ulong(dbRequestId);
        msg.push_ulong(id);
        msg.push_ulong(jobId);
        msg.push_string(what);
        msg.push_uint(state);
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
};

#endif //ADACS_JOB_CLIENT_SSTATUS_H
