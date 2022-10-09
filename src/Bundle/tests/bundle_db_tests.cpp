//
// Created by lewis on 10/9/22.
//


#include "../../tests/fixtures/BundleFixture.h"
#include "../../tests/fixtures/JsonConfigFixture.h"
#include "../BundleManager.h"
#include "../../tests/fixtures/WebsocketServerFixture.h"
#include <boost/test/unit_test.hpp>

struct BundleDbTestFixture : public BundleFixture, public WebsocketServerFixture {
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;

    BundleDbTestFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    ~BundleDbTestFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& connection) {
        switch (message->getId()) {
            case DB_BUNDLE_CREATE_OR_UPDATE_JOB: {
                auto dbRequestId = message->pop_ulong();
                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_ulong(4321);
                result.send(connection);
                return;
            }
        }
    }
};

BOOST_FIXTURE_TEST_SUITE(bundle_db_test_suite, BundleDbTestFixture)
    BOOST_AUTO_TEST_CASE(test_create_or_update_job) {
        auto bundleHash = generateUUID();

        auto job = nlohmann::json {
                {"job_id", 0},
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        writeBundleDbCreateOrUpdateJob(bundleHash, job);

        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the result matches the job with the updated job id
        job["job_id"] = 4321;
        BOOST_CHECK_EQUAL(result, job);
    }
BOOST_AUTO_TEST_SUITE_END()