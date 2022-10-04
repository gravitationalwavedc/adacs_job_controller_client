//
// Created by lewis on 4/10/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../Websocket/WebsocketInterface.h"
#include "../../lib/jobclient_schema.h"
#include "../../DB/SqliteConnector.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../tests/fixtures/BundleFixture.h"
#include "../JobHandling.h"
#include "../../tests/fixtures/DatabaseFixture.h"
#include "../../tests/fixtures/AbortHelperFixture.h"
#include "../../lib/JobStatus.h"

void checkJobStatusImpl(sJob job, bool forceNotification);

struct JobCheckStatusTestDataFixture : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture, public AbortHelperFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    std::string bundleHash = generateUUID();
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobCheckStatusTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    virtual ~JobCheckStatusTestDataFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(std::shared_ptr<TestWsServer::InMessage> message) {
        auto stringData = message->string();
        receivedMessages.push( std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end())));
    }
};

BOOST_FIXTURE_TEST_SUITE(job_check_status_test_suite, JobCheckStatusTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_check_status) {
        auto job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.schedulerId = 4321;
        job.save();
        
        nlohmann::json result = {
                {
                    "status", nlohmann::json::array({

                    })
                }
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, true);

        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never check_statusted the job to the bundle");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), SYSTEM_SOURCE);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::SUBMITTED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job check_statusted successfully");

        // Job should no longer be check_statusting, and should have a scheduler id set
//        BOOST_CHECK_EQUAL(job.check_statusting, false);
//        BOOST_CHECK_EQUAL(job.schedulerId, 4321);
    }
BOOST_AUTO_TEST_SUITE_END()
