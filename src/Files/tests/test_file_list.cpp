//
// Created by lewis on 9/28/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../Websocket/WebsocketInterface.h"
#include "../../lib/jobclient_schema.h"
#include "../../DB/SqliteConnector.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"

struct FileListTestDataFixture : public WebsocketServerFixture, public TemporaryDirectoryFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::shared_ptr<Message> receivedMessage;
    std::promise<void> promMessageReceived;
    uint64_t jobId;
    std::string tempDir = createTemporaryDirectory();
    SqliteConnector database = SqliteConnector();
    schema::JobclientJob jobTable;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    FileListTestDataFixture() {
        // Insert a job in the database
        jobId = database->operator()(
                insert_into(jobTable)
                        .set(
                                jobTable.jobId = 1234,
                                jobTable.bundleHash = "my_hash",
                                jobTable.workingDirectory = tempDir,
                                jobTable.submitting = 0,
                                jobTable.queued = 0,
                                jobTable.running = 0,
                                jobTable.submittingCount = 0,
                                jobTable.params = ""
                        )
        );


        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    virtual ~FileListTestDataFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(std::shared_ptr<TestWsServer::InMessage> message) {
        auto stringData = message->string();
        receivedMessage = std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end()));
        promMessageReceived.set_value();
    }
};

BOOST_FIXTURE_TEST_SUITE(file_list_test_suite, FileListTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_get_file_list_job_not_exist) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(jobId + 1000);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempDir);
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job does not exist");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_submitting) {
        database->operator()(update(jobTable).set(jobTable.submitting = 1).where(jobTable.id == jobId));

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(jobId);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempDir);
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job is not submitted");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_outside_working_directory) {
        database->operator()(update(jobTable).set(jobTable.workingDirectory = "/usr").where(jobTable.id == jobId));

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(jobId);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string("../" + tempDir);
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to list files is outside the working directory");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_directory_not_exist) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(jobId);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempDir + "/not/real/");
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to list files does not exist");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_directory_is_a_file) {
        auto tempFile = createTemporaryFile(tempDir);

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(jobId);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(boost::filesystem::path(tempFile).filename().string());
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to list files is not a directory");
    }
BOOST_AUTO_TEST_SUITE_END()
