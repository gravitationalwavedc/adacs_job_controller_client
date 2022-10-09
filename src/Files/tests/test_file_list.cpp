//
// Created by lewis on 9/28/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../lib/jobclient_schema.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../tests/fixtures/BundleFixture.h"
#include "../../tests/fixtures/DatabaseFixture.h"

struct FileListTestDataFixture : public WebsocketServerFixture, public TemporaryDirectoryFixture, public BundleFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::shared_ptr<Message> receivedMessage;
    std::promise<void> promMessageReceived;
    uint64_t jobId;
    std::string symlinkDir = createTemporaryDirectory();
    std::string symlinkFile = createTemporaryFile(symlinkDir);
    std::string tempDir = createTemporaryDirectory();
    std::string tempFile = createTemporaryFile(tempDir);
    std::string tempDir2 = createTemporaryDirectory(tempDir);
    std::string tempFile2 = createTemporaryFile(tempDir2);
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    FileListTestDataFixture() {
        // Create symlinks
        boost::filesystem::create_directory_symlink(symlinkDir, boost::filesystem::path(tempDir) / "symlink_dir");
        boost::filesystem::create_symlink(symlinkFile, boost::filesystem::path(tempDir) / "symlink_path");
        boost::filesystem::create_directory_symlink("/not/a/real/path", boost::filesystem::path(tempDir) / "symlink_dir_not_real");
        boost::filesystem::create_symlink("/not/a/real/path", boost::filesystem::path(tempDir) / "symlink_path_not_real");

        // Generate some fake data
        std::ofstream ofs1(tempFile);
        ofs1 << "12345";
        ofs1.close();

        std::ofstream ofs2(tempFile2);
        ofs2 << "12345678";
        ofs2.close();

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

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& connection) {
        receivedMessage = message;
        promMessageReceived.set_value();
    }
};

BOOST_FIXTURE_TEST_SUITE(file_list_test_suite, FileListTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_get_file_list_job_not_exist) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234 + 1000);
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
        msg.push_int(1234);
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
        msg.push_int(1234);
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
        msg.push_int(1234);
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
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
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

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_success_recursive) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string("/");
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_uint(), 3);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempDir2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), true);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 0);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 8);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 5);
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_success_recursive_2) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempDir2.substr(tempDir.length()));
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_uint(), 1);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 8);
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_success_not_recursive) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string("/");
        msg.push_bool(false);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_uint(), 2);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempDir2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), true);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 0);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 5);
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_job_success_not_recursive_2) {
        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempDir2.substr(tempDir.length()));
        msg.push_bool(false);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_uint(), 1);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 8);
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_no_job_outside_working_directory) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, "/usr");

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string("../" + tempDir);
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to list files is outside the working directory");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_no_job_directory_not_exist) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, tempDir);

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string(tempDir + "/not/real/");
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to list files does not exist");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_no_job_directory_is_a_file) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, tempDir);

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string(boost::filesystem::path(tempFile).filename().string());
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to list files is not a directory");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_list_no_job_success) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, tempDir);

        Message msg(FILE_LIST, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string("/");
        msg.push_bool(true);
        msg.send(pWebsocketServerConnection);

        promMessageReceived.get_future().wait();

        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_LIST);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_uint(), 3);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempDir2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), true);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 0);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile2.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 8);

        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), tempFile.substr(tempDir.length()));
        BOOST_CHECK_EQUAL(receivedMessage->pop_bool(), false);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), 5);
    }
BOOST_AUTO_TEST_SUITE_END()
