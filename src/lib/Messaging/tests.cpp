//
// Created by lewis on 2/26/20.
//
#include "../../Websocket/WebsocketInterface.h"
#include "Message.h"
#include <boost/test/unit_test.hpp>
#include <chrono>
#include <random>

// NOLINTBEGIN(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers,misc-non-private-member-variables-in-classes)

class MessageTestFixture {
private:
    // Define an override cluster that can be used to mutate a message
    class TestWebsocketInterface : public WebsocketInterface {
    public:
        TestWebsocketInterface() = default;

        std::shared_ptr<std::vector<uint8_t>> vData;
        std::string sSource;
        Message::Priority ePriority = Message::Priority::Lowest;

    private:
        void queueMessage(std::string source, const std::shared_ptr<std::vector<uint8_t>>& data, Message::Priority priority, std::function<void()> callback = [] {}) override {
            vData = data;
            sSource = source;
            ePriority = priority;
        }
    };
public:
    std::shared_ptr<TestWebsocketInterface> testWebsocketInterface;

    MessageTestFixture() {
        testWebsocketInterface = std::make_shared<TestWebsocketInterface>(); // NOLINT(cert-err58-cpp)
        WebsocketInterface::setSingleton(testWebsocketInterface);
    }
};

BOOST_FIXTURE_TEST_SUITE(Message_test_suite, MessageTestFixture)
    BOOST_AUTO_TEST_CASE(test_message_attributes) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.getId(), 0);
        BOOST_CHECK_EQUAL(testWebsocketInterface->ePriority, Message::Priority::Highest);
        BOOST_CHECK_EQUAL(testWebsocketInterface->sSource, "test");

        msg = Message(101, Message::Priority::Highest, "test");

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.getId(), 101);
        BOOST_CHECK_EQUAL(testWebsocketInterface->ePriority, Message::Priority::Highest);
        BOOST_CHECK_EQUAL(testWebsocketInterface->sSource, "test");

        msg = Message(0, Message::Priority::Medium, "test");

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.getId(), 0);
        BOOST_CHECK_EQUAL(testWebsocketInterface->ePriority, Message::Priority::Medium);
        BOOST_CHECK_EQUAL(testWebsocketInterface->sSource, "test");

        msg = Message(0, Message::Priority::Highest, "test_again");

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.getId(), 0);
        BOOST_CHECK_EQUAL(testWebsocketInterface->ePriority, Message::Priority::Highest);
        BOOST_CHECK_EQUAL(testWebsocketInterface->sSource, "test_again");

        msg = Message(123, Message::Priority::Lowest, "test_1");

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.getId(), 123);
        BOOST_CHECK_EQUAL(testWebsocketInterface->ePriority, Message::Priority::Lowest);
        BOOST_CHECK_EQUAL(testWebsocketInterface->sSource, "test_1");
    }

    BOOST_AUTO_TEST_CASE(test_primitive_bool) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_bool(true);
        msg.push_bool(false);
        msg.push_bool(true);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_bool(), true);
        BOOST_CHECK_EQUAL(msg.pop_bool(), false);
        BOOST_CHECK_EQUAL(msg.pop_bool(), true);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_ubyte) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_ubyte(1);
        msg.push_ubyte(5);
        msg.push_ubyte(245);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_ubyte(), 1);
        BOOST_CHECK_EQUAL(msg.pop_ubyte(), 5);
        BOOST_CHECK_EQUAL(msg.pop_ubyte(), 245);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_byte) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_byte(1);
        msg.push_byte(-120);
        msg.push_byte(120);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_byte(), 1);
        BOOST_CHECK_EQUAL(msg.pop_byte(), -120);
        BOOST_CHECK_EQUAL(msg.pop_byte(), 120);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_ushort) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_ushort(0x1);
        msg.push_ushort(0x1200);
        msg.push_ushort(0xffff);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_ushort(), 0x1);
        BOOST_CHECK_EQUAL(msg.pop_ushort(), 0x1200);
        BOOST_CHECK_EQUAL(msg.pop_ushort(), 0xffff);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_short) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_short(0x1);
        msg.push_short(-0x1200);
        msg.push_short(0x2020);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_short(), 0x1);
        BOOST_CHECK_EQUAL(msg.pop_short(), -0x1200);
        BOOST_CHECK_EQUAL(msg.pop_short(), 0x2020);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_uint) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_uint(0x1);
        msg.push_uint(0x12345678);
        msg.push_uint(0xffff1234);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_uint(), 0x1);
        BOOST_CHECK_EQUAL(msg.pop_uint(), 0x12345678);
        BOOST_CHECK_EQUAL(msg.pop_uint(), 0xffff1234);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_int) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_int(0x1);
        msg.push_int(-0x12345678);
        msg.push_int(0x12345678);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_int(), 0x1);
        BOOST_CHECK_EQUAL(msg.pop_int(), -0x12345678);
        BOOST_CHECK_EQUAL(msg.pop_int(), 0x12345678);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_ulong) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_ulong(0x1);
        msg.push_ulong(0x1234567812345678);
        msg.push_ulong(0xffff123412345678);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_ulong(), 0x1);
        BOOST_CHECK_EQUAL(msg.pop_ulong(), 0x1234567812345678);
        BOOST_CHECK_EQUAL(msg.pop_ulong(), 0xffff123412345678);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_long) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_long(0x1);
        msg.push_long(-0x1234567812345678);
        msg.push_long(0x1234567812345678);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_long(), 0x1);
        BOOST_CHECK_EQUAL(msg.pop_long(), -0x1234567812345678);
        BOOST_CHECK_EQUAL(msg.pop_long(), 0x1234567812345678);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_float, *boost::unit_test::tolerance(0.00001)) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_float(0.1F);
        msg.push_float(0.1234567812345678F);
        msg.push_float(-0.1234123F);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_float(), 0.1F);
        BOOST_CHECK_EQUAL(msg.pop_float(), 0.1234567812345678F);
        BOOST_CHECK_EQUAL(msg.pop_float(), -0.1234123F);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_double, *boost::unit_test::tolerance(0.00001)) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        msg.push_double(0.1);
        msg.push_double(0.1234567812345678);
        msg.push_double(-0.1234567812345678);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_double(), 0.1);
        BOOST_CHECK_EQUAL(msg.pop_double(), 0.1234567812345678);
        BOOST_CHECK_EQUAL(msg.pop_double(), -0.1234567812345678);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_string) {
        auto msg = Message(0, Message::Priority::Highest, "test");

        auto str1 = std::string("Hello!");
        auto str2 = std::string("Hello again!");
        auto str3 = std::string("Hello one last time!");

        msg.push_string(str1);
        msg.push_string(str2);
        msg.push_string(str3);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        BOOST_CHECK_EQUAL(msg.pop_string(), str1);
        BOOST_CHECK_EQUAL(msg.pop_string(), str2);
        BOOST_CHECK_EQUAL(msg.pop_string(), str3);
    }

    BOOST_AUTO_TEST_CASE(test_primitive_bytes) {
        std::random_device engine;

        std::vector<uint8_t> data;
        data.reserve(512);
        for (auto i = 0; i < 512; i++) {
            data.push_back(engine());
        }

        auto msg = Message(0, Message::Priority::Highest, "test");
        msg.push_bytes(data);

        // Transmute the message into a received message
        msg.send();
        msg = Message(*testWebsocketInterface->vData);

        auto result = msg.pop_bytes();

        BOOST_CHECK_EQUAL_COLLECTIONS(data.begin(), data.end(), result.begin(), result.end());
    }

    auto get_millis() -> std::chrono::milliseconds {
        return std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::system_clock::now().time_since_epoch()
        );
    }

    BOOST_AUTO_TEST_CASE(test_primitive_bytes_rate) {
        std::random_device engine;

        // First try 512b chunks
        auto time_now = get_millis();
        auto time_next = time_now + std::chrono::seconds(5);

        std::vector<uint8_t> data;
        data.reserve(1024ULL * 1024ULL);
        for (auto i = 0; i < 512; i++) {
            data.push_back(engine());
        }

        {
            auto msg = Message(0, Message::Priority::Highest, "test");
            msg.push_bytes(data);

            // Transmute the message into a received message
            msg.send();
            msg = Message(*testWebsocketInterface->vData);

            auto result = msg.pop_bytes();

            BOOST_CHECK_EQUAL_COLLECTIONS(data.begin(), data.end(), result.begin(), result.end());
        }

        uint64_t counter = 0;
        while (get_millis() < time_next) {
            auto msg = Message(0, Message::Priority::Highest, "test");
            msg.push_bytes(data);

            // Transmute the message into a received message
            msg.send();
            msg = Message(*testWebsocketInterface->vData);

            auto result = msg.pop_bytes();
            counter += result.size();
        }

        std::cout << "Message raw bytes throughput 512b chunks Mb/s: " << static_cast<float>(counter) / 1024.F / 1024.F / 5 << std::endl;

        // Now try 1Mb chunks
        time_now = get_millis();
        time_next = time_now + std::chrono::seconds(5);

        data.clear();
        for (auto i = 0; i < 1024 * 1024; i++) {
            data.push_back(engine());
        }

        counter = 0;
        while (get_millis() < time_next) {
            auto msg = Message(0, Message::Priority::Highest, "test");
            msg.push_bytes(data);

            // Transmute the message into a received message
            msg.send();
            msg = Message(*testWebsocketInterface->vData);

            auto result = msg.pop_bytes();
            counter += result.size();
        }

        std::cout << "Message raw bytes throughput 1Mb chunks Mb/s: " << static_cast<float>(counter) / 1024.F / 1024.F / 5 << std::endl;
    }

BOOST_AUTO_TEST_SUITE_END()

// NOLINTEND(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers,misc-non-private-member-variables-in-classes)