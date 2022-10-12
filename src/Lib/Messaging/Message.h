//
// Created by lewis on 2/26/20.
//

#ifndef GWCLOUD_JOB_SERVER_MESSAGE_H
#define GWCLOUD_JOB_SERVER_MESSAGE_H

#include "../GeneralUtils.h"
#include "server_ws.hpp"
#include <cstdint>
#include <deque>
#include <string>
#include <vector>

#ifdef BUILD_TESTS
#include "../../Tests/utils.h"
#endif

constexpr const char* SYSTEM_SOURCE = "system";

constexpr uint32_t SERVER_READY = 1000;

constexpr uint32_t SUBMIT_JOB = 2000;
constexpr uint32_t UPDATE_JOB = 2001;
constexpr uint32_t CANCEL_JOB = 2002;
constexpr uint32_t DELETE_JOB = 2003;

constexpr uint32_t FILE_DOWNLOAD = 4000;
constexpr uint32_t FILE_DOWNLOAD_DETAILS = 4001;
constexpr uint32_t FILE_DOWNLOAD_ERROR = 4002;
constexpr uint32_t FILE_CHUNK = 4003;
constexpr uint32_t PAUSE_FILE_CHUNK_STREAM = 4004;
constexpr uint32_t RESUME_FILE_CHUNK_STREAM = 4005;
constexpr uint32_t FILE_LIST = 4006;
constexpr uint32_t FILE_LIST_ERROR = 4007;

constexpr uint32_t DB_JOB_GET_BY_JOB_ID = 5000;
constexpr uint32_t DB_JOB_GET_BY_ID = 5001;
constexpr uint32_t DB_JOB_GET_RUNNING_JOBS = 5002;
constexpr uint32_t DB_JOB_DELETE = 5003;
constexpr uint32_t DB_JOB_SAVE = 5004;

constexpr uint32_t DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT = 6000;
constexpr uint32_t DB_JOBSTATUS_GET_BY_JOB_ID = 6001;
constexpr uint32_t DB_JOBSTATUS_DELETE_BY_ID_LIST = 6002;
constexpr uint32_t DB_JOBSTATUS_SAVE = 6003;

constexpr uint32_t DB_RESPONSE = 7000;

constexpr uint32_t DB_BUNDLE_CREATE_OR_UPDATE_JOB = 8000;
constexpr uint32_t DB_BUNDLE_GET_JOB_BY_ID = 8001;
constexpr uint32_t DB_BUNDLE_DELETE_JOB = 8002;

class Message {
public:
    enum Priority {
        Lowest = 19,
        Medium = 10,
        Highest = 0
    };

#ifdef BUILD_TESTS
    explicit Message(uint32_t msgId);
#endif

    Message(uint32_t msgId, Priority priority, const std::string& source, std::function<void()>  callback = [] {});
    explicit Message(const std::vector<uint8_t>& vdata);

    void push_bool(bool value);
    void push_ubyte(uint8_t value);
    void push_byte(int8_t value);
    void push_ushort(uint16_t value);
    void push_short(int16_t value);
    void push_uint(uint32_t value);
    void push_int(int32_t value);
    void push_ulong(uint64_t value);
    void push_long(int64_t value);
    void push_float(float value);
    void push_double(double value);
    void push_string(const std::string& value);
    void push_bytes(const std::vector<uint8_t>& value);

    auto pop_bool() -> bool;
    auto pop_ubyte() -> uint8_t;
    auto pop_byte() -> int8_t;
    auto pop_ushort() -> uint16_t;
    auto pop_short() -> int16_t;
    auto pop_uint() -> uint32_t;
    auto pop_int() -> int32_t;
    auto pop_ulong() -> uint64_t;
    auto pop_long() -> int64_t;
    auto pop_float() -> float;
    auto pop_double() -> double;
    auto pop_string() -> std::string;
    auto pop_bytes() -> std::vector<uint8_t>;

    void send();

#ifdef BUILD_TESTS
    void send(const std::shared_ptr<TestWsServer::Connection>& connection);
#endif

    [[nodiscard]] auto getId() const -> uint32_t { return id; }

private:
    std::shared_ptr<std::vector<uint8_t>> data;
    uint64_t index;
    Priority priority = Priority::Lowest;
    std::string source;
    uint32_t id = 0;
    std::function<void()> callback = [] {};

EXPOSE_PROPERTY_FOR_TESTING(data);
};

#endif //GWCLOUD_JOB_SERVER_MESSAGE_H
