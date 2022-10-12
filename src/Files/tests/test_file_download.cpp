//
// Created by lewis on 9/28/22.
//

#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"

extern std::map<std::string, std::promise<void>> pausedFileTransfers;

struct FileDownloadTestDataFixture : public WebsocketServerFixture, public TemporaryDirectoryFixture, public BundleFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    uint64_t jobId;
    std::string symlinkDir = createTemporaryDirectory();
    std::string symlinkFile = createTemporaryFile(symlinkDir);
    std::string tempDir = createTemporaryDirectory();
    std::string tempFile = createTemporaryFile(tempDir);
    std::string tempDir2 = createTemporaryDirectory(tempDir);
    std::string tempFile2 = createTemporaryFile(tempDir2);
    std::chrono::time_point<std::chrono::system_clock> lastMessageTime;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    FileDownloadTestDataFixture() {
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

    ~FileDownloadTestDataFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& /*connection*/) override {
        receivedMessages.push(message);
        lastMessageTime = std::chrono::system_clock::now();
    }

    [[nodiscard]] auto writeFileData() const -> std::shared_ptr<std::vector<uint8_t>> {
        // Write out some random data to one of the temp files
        std::ofstream file(tempFile2, std::ios::trunc | std::ios::binary);

        // Data size between 64Mb and 128Mb
        auto dataLength = randomInt(1024ULL*1024ULL*64ULL, 1024ULL*1024ULL*128ULL);
        auto fileData = generateRandomData(dataLength);

        // NOLINTNEXTLINE(cppcoreguidelines-pro-type-reinterpret-cast)
        file.write(reinterpret_cast<char*>(fileData->data()), static_cast<std::streamsize>(fileData->size()));
        file.flush();
        file.close();

        return fileData;
    }

    void doFileDownload(const std::shared_ptr<std::vector<uint8_t>>& fileData, const std::string& downloadUuid) { // NOLINT(readability-function-cognitive-complexity)
        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_DETAILS);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), downloadUuid);
        BOOST_CHECK_EQUAL(receivedMessage->pop_ulong(), fileData->size());

        // Clean up the front of the queue
        receivedMessages.pop();

        // Wait for the entire file to be sent
        std::vector<uint8_t> dataReceived;
        dataReceived.reserve(fileData->size());

        auto bPaused = false;
        auto bPauseChecked = false;
        std::chrono::time_point<std::chrono::system_clock> pauseTime;

        while (dataReceived.size() < fileData->size()) {
            // Spin waiting for the next message
            while (receivedMessages.empty()) {
                if (bPauseChecked) {
                    continue;
                }

                // If we've been paused for more than a second, check that the client didn't continue sending packets
                auto now = std::chrono::system_clock::now();
                if (bPaused && std::chrono::duration_cast<std::chrono::milliseconds>(now - pauseTime).count() > 1000) {
                    BOOST_CHECK_MESSAGE(
                            std::chrono::duration_cast<std::chrono::milliseconds>(now - lastMessageTime).count() > 500,
                            "Client should have stopped sending packets while paused"
                    );

                    // Resume the transfer
                    Message msg(RESUME_FILE_CHUNK_STREAM, Message::Priority::Highest, SYSTEM_SOURCE);
                    msg.push_string(downloadUuid);
                    msg.send(pWebsocketServerConnection);

                    bPauseChecked = true;
                }
            }

            auto msg = receivedMessages.front();
            if (msg->pop_string() != downloadUuid) {
                BOOST_FAIL("Download UUID didn't match");
                return;
            }

            auto data = msg->pop_bytes();
            dataReceived.insert(dataReceived.end(), data.begin(), data.end());

            receivedMessages.pop();

            // Test pausing if we have to
            if (!bPaused) {
                bPaused = true;
                Message msg(PAUSE_FILE_CHUNK_STREAM, Message::Priority::Highest, SYSTEM_SOURCE);
                msg.push_string(downloadUuid);
                msg.send(pWebsocketServerConnection);
                pauseTime = std::chrono::system_clock::now();
            }
        }

        BOOST_CHECK_EQUAL_COLLECTIONS(dataReceived.begin(), dataReceived.end(), fileData->begin(), fileData->end());
    }
};

BOOST_FIXTURE_TEST_SUITE(file_download_test_suite, FileDownloadTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_get_file_download_job_not_exist) {
        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234 + 1000);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempFile);
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job does not exist");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_job_submitting) {
        database->operator()(update(jobTable).set(jobTable.submitting = 1).where(jobTable.id == jobId));

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempFile);
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job is not submitted");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_job_outside_working_directory) {
        database->operator()(update(jobTable).set(jobTable.workingDirectory = "/usr").where(jobTable.id == jobId));

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string("../" + tempFile);
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to file download is outside the working directory");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_job_file_not_exist) {
        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(tempFile + ".not.real");
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to file download does not exist");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_job_file_is_a_directory) {
        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string("some_uuid");
        msg.push_string("some_bundle_hash");
        msg.push_string(boost::filesystem::path(tempDir2).filename().string());
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to file download is not a file");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_no_job_outside_working_directory) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, "/usr");

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string("../" + tempFile);
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to file download is outside the working directory");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_no_job_directory_not_exist) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, tempDir);

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string(tempFile + ".not.real");
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to file download does not exist");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_no_job_file_is_a_directory) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, tempDir);

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string("some_uuid");
        msg.push_string(bundleHash);
        msg.push_string(boost::filesystem::path(tempDir2).filename().string());
        msg.send(pWebsocketServerConnection);

        // Spin waiting for the next message
        while (receivedMessages.empty()) {}

        auto receivedMessage = receivedMessages.front();
        BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_DOWNLOAD_ERROR);
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "some_uuid");
        BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Path to file download is not a file");
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_job_success) {
        auto fileData = writeFileData();

        auto downloadUuid = generateUUID();

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(1234);
        msg.push_string(downloadUuid);
        msg.push_string("some_bundle_hash");
        msg.push_string(tempFile2.substr(tempDir.length()));
        msg.send(pWebsocketServerConnection);

        doFileDownload(fileData, downloadUuid);
    }

    BOOST_AUTO_TEST_CASE(test_get_file_download_no_job_success) {
        auto bundleHash = generateUUID();
        writeFileListNoJobWorkingDirectory(bundleHash, tempDir);

        auto fileData = writeFileData();

        auto downloadUuid = generateUUID();

        Message msg(FILE_DOWNLOAD, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.push_int(0);
        msg.push_string(downloadUuid);
        msg.push_string(bundleHash);
        msg.push_string(tempFile2.substr(tempDir.length()));
        msg.send(pWebsocketServerConnection);

        doFileDownload(fileData, downloadUuid);
    }
BOOST_AUTO_TEST_SUITE_END()
