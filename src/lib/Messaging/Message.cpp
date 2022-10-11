//
// Created by lewis on 2/26/20.
//

#include "Message.h"

#include "../../Settings.h"
#include "../../Websocket/WebsocketInterface.h"
#include <utility>

#ifdef BUILD_TESTS
Message::Message(uint32_t msgId) : index(0), id(msgId) {
    // Constructor only used for testing
    data = std::make_shared<std::vector<uint8_t>>();
    data->reserve(MESSAGE_INITIAL_VECTOR_SIZE);
}
#endif

Message::Message(uint32_t msgId, Message::Priority priority, const std::string& source, std::function<void()>  callback)
: priority(priority), index(0), source(source), callback(std::move(callback)) {
    data = std::make_shared<std::vector<uint8_t>>();
    data->reserve(MESSAGE_INITIAL_VECTOR_SIZE);

    // Push the source
    push_string(source);

    // Push the id
    push_uint(msgId);
}

Message::Message(const std::vector<uint8_t>& vdata) :
data(std::make_shared<std::vector<uint8_t>>(vdata)), index(0), source(pop_string()), id(pop_uint()) {}

void Message::push_bool(bool value) {
    push_ubyte(value ? 1 : 0);
}

auto Message::pop_bool() -> bool {
    auto result = pop_ubyte();
    return result == 1;
}

void Message::push_ubyte(uint8_t value) {
    data->push_back(value);
}

auto Message::pop_ubyte() -> uint8_t {
    auto result = (*data)[index++];
    return result;
}

void Message::push_byte(int8_t value) {
    push_ubyte(static_cast<uint8_t>(value));
}

auto Message::pop_byte() -> int8_t {
    return static_cast<int8_t>(pop_ubyte());
}

// NOLINTBEGIN(cppcoreguidelines-macro-usage)
#define push_type(t, r) void Message::push_##t (r value) {      \
    std::array<uint8_t, sizeof(value)> data{};                  \
                                                                \
    memcpy(data.data(), &value, sizeof(value));                 \
                                                                \
    for (unsigned char nextbyte : data) {                       \
        push_ubyte(nextbyte);                                   \
    }                                                           \
}

#define pop_type(t, r) auto Message::pop_##t() -> r {           \
    std::array<uint8_t, sizeof(r)> data{};                      \
                                                                \
    for (auto index = 0; index < sizeof(r); index++) {          \
        data.at(index) = pop_ubyte();                           \
    }                                                           \
                                                                \
    r result;                                                   \
    memcpy(&result, data.data(), sizeof(r));                    \
    return result;                                              \
}

#define add_type(t, r) push_type(t, r) pop_type(t, r)
// NOLINTEND(cppcoreguidelines-macro-usage)

add_type(ushort, uint16_t)

add_type(short, int16_t)

add_type(uint, uint32_t)

add_type(int, int32_t)

add_type(ulong, uint64_t)

add_type(long, int64_t)

add_type(float, float)

add_type(double, double)

void Message::push_string(const std::string& value) {
    push_ulong(value.size());
    data->insert(data->end(), value.begin(), value.end());
}

auto Message::pop_string() -> std::string {
    auto result = pop_bytes();
    return {result.begin(), result.end()};
}

void Message::push_bytes(const std::vector<uint8_t>& value) {
    push_ulong(value.size());
    data->insert(data->end(), value.begin(), value.end());
}

auto Message::pop_bytes() -> std::vector<uint8_t> {
    auto len = pop_ulong();
    auto result = std::vector<uint8_t>(
        data->begin() + static_cast<int64_t>(index),
        data->begin() + static_cast<int64_t>(index) + static_cast<int64_t>(len)
    );
    index += len;
    return result;
}

void Message::send() {
    WebsocketInterface::Singleton()->queueMessage(source, data, priority, callback);
}

#ifdef BUILD_TESTS
void Message::send(const std::shared_ptr<TestWsServer::Connection>& connection) {
    auto outMessage = std::make_shared<TestWsServer::OutMessage>(data->size());
    std::copy(data->begin(), data->end(), std::ostream_iterator<uint8_t>(*outMessage));
    connection->send(outMessage);
}
#endif
