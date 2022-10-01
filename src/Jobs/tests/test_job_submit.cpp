//
// Created by lewis on 9/28/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../Websocket/WebsocketInterface.h"
#include "../../lib/jobclient_schema.h"
#include "../../DB/SqliteConnector.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../tests/fixtures/BundleFixture.h"
#include "../JobHandling.h"
#include "../../tests/fixtures/DatabaseFixture.h"

struct JobSubmitTestDataFixture : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    std::string bundleHash = generateUUID();
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobSubmitTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    virtual ~JobSubmitTestDataFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(std::shared_ptr<TestWsServer::InMessage> message) {
        auto stringData = message->string();
        receivedMessages.push( std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end())));
    }
};

BOOST_FIXTURE_TEST_SUITE(job_submit_test_suite, JobSubmitTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_submit_timeout) {
        auto job = sJob::getOrCreateByJobId(1234);
        job.submitting = true;
        job.save();

        for (int count = 1; count < 10; count++) {
            Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
            msg.push_int(job.jobId);
            msg.push_string(bundleHash);
            msg.push_string("test params");
            msg.send(pWebsocketServerConnection);

            int retry = 0;
            for (; retry < 10 && job.submittingCount != count; retry++) {
                std::this_thread::sleep_for(std::chrono::milliseconds(100));
                job.refreshFromDb();
            }

            BOOST_CHECK_EQUAL(job.submittingCount, count);

            if (retry == 10) {
                BOOST_FAIL("Client never incremented submittingCount");
            }
        }
    }
BOOST_AUTO_TEST_SUITE_END()
