//
// Created by lewis on 10/9/22.
//


#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/JsonConfigFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"
#include "../BundleManager.h"
#include <boost/test/unit_test.hpp>

struct BundleDbTestFixture : public BundleFixture, public WebsocketServerFixture {
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    nlohmann::json getJobByIdResult;
    uint64_t getJobByIdRequestId = 0;
    uint64_t deletedJobId = 0;
    std::string bundleHashRequest;
    bool returnSuccess = true;

    BundleDbTestFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    ~BundleDbTestFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& connection) override {
        switch (message->getId()) {
            case DB_BUNDLE_CREATE_OR_UPDATE_JOB: {
                auto dbRequestId = message->pop_ulong();
                bundleHashRequest = message->pop_string();

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(returnSuccess);
                result.push_ulong(4321);
                result.send(connection);
                return;
            }
            case DB_BUNDLE_GET_JOB_BY_ID: {
                auto dbRequestId = message->pop_ulong();
                bundleHashRequest = message->pop_string();
                getJobByIdRequestId = message->pop_ulong();

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(returnSuccess);
                result.push_ulong(4321);
                result.push_string(getJobByIdResult.dump());
                result.send(connection);
                return;
            }
            case DB_BUNDLE_DELETE_JOB: {
                auto dbRequestId = message->pop_ulong();
                bundleHashRequest = message->pop_string();
                deletedJobId = message->pop_ulong();

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(returnSuccess);
                result.send(connection);
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
        BOOST_CHECK_EQUAL(bundleHashRequest, bundleHash);
    }

    BOOST_AUTO_TEST_CASE(test_create_or_update_job_failure) {
        auto bundleHash = generateUUID();

        auto job = nlohmann::json {
                {"job_id", 0},
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        writeBundleDbCreateOrUpdateJob(bundleHash, job);

        returnSuccess = false;
        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the correct exception was thrown
        BOOST_CHECK_EQUAL(result["error"], "Job was unable to be created or updated.");
        BOOST_CHECK_EQUAL(bundleHashRequest, bundleHash);
    }

    BOOST_AUTO_TEST_CASE(get_job_by_id) {
        auto bundleHash = generateUUID();

        getJobByIdResult = nlohmann::json {
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        writeBundleDbGetJobById(bundleHash, 4321);

        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the result matches the job with the updated job id
        getJobByIdResult["job_id"] = 4321;
        BOOST_CHECK_EQUAL(result, getJobByIdResult);
        BOOST_CHECK_EQUAL(getJobByIdRequestId, 4321);
        BOOST_CHECK_EQUAL(bundleHashRequest, bundleHash);
    }

    BOOST_AUTO_TEST_CASE(get_job_by_id_failure) {
        auto bundleHash = generateUUID();

        getJobByIdResult = nlohmann::json {
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        returnSuccess = false;
        writeBundleDbGetJobById(bundleHash, 4321);

        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the correct exception was thrown
        BOOST_CHECK_EQUAL(result["error"], "Job with ID 4321 does not exist.");
        BOOST_CHECK_EQUAL(bundleHashRequest, bundleHash);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job) {
        auto bundleHash = generateUUID();

        auto job = nlohmann::json {
                {"job_id", 4321},
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        writeBundleDbDeleteJob(bundleHash, job);

        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the job id requested for deletion was expected
        BOOST_CHECK_EQUAL(deletedJobId, 4321);
        BOOST_CHECK_EQUAL(bundleHashRequest, bundleHash);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_failure_job_id_must_be_provided) {
        auto bundleHash = generateUUID();

        // Check without job id present in dictionary
        auto job = nlohmann::json {
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        writeBundleDbDeleteJob(bundleHash, job);

        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the correct exception was thrown
        BOOST_CHECK_EQUAL(result["error"], "Job ID must be provided.");

        // Bundle hash shouldn't have been set, since the job id check happens before sending the websocket message
        BOOST_CHECK_EQUAL(bundleHashRequest.empty(), true);

        // Check that if job id is 0, the same exception is thrown
        job["job_id"] = 0;

        result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the correct exception was thrown
        BOOST_CHECK_EQUAL(result["error"], "Job ID must be provided.");

        // Bundle hash shouldn't have been set, since the job id == 0 check happens before sending the websocket message
        BOOST_CHECK_EQUAL(bundleHashRequest.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_failure_job_not_found) {
        auto bundleHash = generateUUID();

        auto job = nlohmann::json {
                {"job_id", 4321},
                {"submit_id", 1234},
                {"working_directory", "/test/working/directory"},
                {"submit_directory", "/test/working/directory/submit"}
        };

        writeBundleDbDeleteJob(bundleHash, job);

        returnSuccess = false;
        auto result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        result = BundleManager::Singleton()->runBundle_json("submit", bundleHash, {}, "");

        // Check that the correct exception was thrown
        BOOST_CHECK_EQUAL(result["error"], "Job with ID 4321 does not exist.");
        BOOST_CHECK_EQUAL(bundleHashRequest, bundleHash);
    }
BOOST_AUTO_TEST_SUITE_END()