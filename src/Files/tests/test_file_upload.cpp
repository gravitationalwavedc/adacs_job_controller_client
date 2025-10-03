//
// Created by lewis on 12/19/24.
//

#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"
#include <queue>

struct FileUploadTestDataFixture : public WebsocketServerFixture, public TemporaryDirectoryFixture, public BundleFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    uint64_t jobId;
    std::string tempDir = createTemporaryDirectory();
    std::string tempFile = createTemporaryFile(tempDir);
    std::string targetFile = tempDir + "/uploaded_file.txt";
    std::chrono::time_point<std::chrono::system_clock> lastMessageTime;
    uint16_t dynamicPort = 0;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    FileUploadTestDataFixture() {
        // Initialize WebsocketInterface singleton (crucial for tests to work)
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        // Generate some test data
        std::ofstream ofs(tempFile);
        ofs << "Test file content for upload";
        ofs.close();

        // Insert a job in the database
        jobId = database->operator()(
                insert_into(jobTable)
                        .set(
                                jobTable.jobId = 1234,
                                jobTable.bundleHash = "upload_test_hash",
                                jobTable.workingDirectory = tempDir,
                                jobTable.submitting = 0,
                                jobTable.running = 0,
                                jobTable.submittingCount = 0,
                                jobTable.deleting = 0,
                                jobTable.deleted = 0
                        )
        );

        // Set up the message handler
        websocketServer->endpoint["^(.*?)$"].on_message = [this](const std::shared_ptr<TestWsServer::Connection>& connection, const std::shared_ptr<TestWsServer::InMessage>& message) {
            handleMessage(connection, message);
        };

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    ~FileUploadTestDataFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    // Track file upload state for each UUID
    struct FileUploadState {
        std::string uuid;
        std::string targetPath;
        uint64_t expectedSize;
        std::vector<uint8_t> receivedData;
        bool simulateError = false;
        std::string errorMessage;
    };
    std::map<std::string, FileUploadState> uploadStates;

    // Store test data to be sent for each UUID
    std::map<std::string, std::string> testDataForUuid;
    std::map<std::string, std::vector<uint8_t>> testBinaryDataForUuid;
    std::map<std::string, std::string> errorMessagesForUuid;

    void handleMessage(const std::shared_ptr<TestWsServer::Connection>& connection, const std::shared_ptr<TestWsServer::InMessage>& message) {
        lastMessageTime = std::chrono::system_clock::now();
        auto data = message->string();
        auto msg = std::make_shared<Message>(std::vector<uint8_t>(data.begin(), data.end()));

        receivedMessages.push(msg);

        // Handle specific message types for upload simulation
        switch (msg->getId()) {
            case SERVER_READY:
                // This is the client confirming it's ready to receive file data
                // We should respond by sending the file chunks for the specific UUID
                {
                    auto uuid = msg->pop_string();
                    sendTestFileData(uuid, connection);
                }
                break;
            case FILE_UPLOAD_COMPLETE:
                // This is the client confirming successful receipt of all data
                {
                    auto uuid = msg->pop_string();
                    // Test completed successfully - nothing more to do
                }
                break;
            default:
                break;
        }
    }

    void sendTestFileData(const std::string& uuid, const std::shared_ptr<TestWsServer::Connection>& connection) {
        // Check if we should send an error for this UUID
        if (errorMessagesForUuid.find(uuid) != errorMessagesForUuid.end()) {
            auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
            errorMsg.push_string(errorMessagesForUuid[uuid]);
            sendResponseMessage(errorMsg, connection);
            return;
        }

        // Send binary data if available
        if (testBinaryDataForUuid.find(uuid) != testBinaryDataForUuid.end()) {
            const auto& data = testBinaryDataForUuid[uuid];
            
            // Send the data as a chunk
            auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uuid);
            chunkMsg.push_bytes(data);
            sendResponseMessage(chunkMsg, connection);
            
            // Send completion message
            auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uuid);
            sendResponseMessage(completeMsg, connection);
            return;
        }

        // Send text data if available
        if (testDataForUuid.find(uuid) != testDataForUuid.end()) {
            const auto& testData = testDataForUuid[uuid];
            
            // Send the data as a chunk
            auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uuid);
            chunkMsg.push_bytes(std::vector<uint8_t>(testData.begin(), testData.end()));
            sendResponseMessage(chunkMsg, connection);
            
            // Send completion message
            auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uuid);
            sendResponseMessage(completeMsg, connection);
            return;
        }

        // Default: send no data (for zero-byte file tests)
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uuid);
        sendResponseMessage(completeMsg, connection);
    }

    void setTestDataForUuid(const std::string& uuid, const std::string& data) {
        testDataForUuid[uuid] = data;
    }

    void setTestBinaryDataForUuid(const std::string& uuid, const std::vector<uint8_t>& data) {
        testBinaryDataForUuid[uuid] = data;
    }

    void setErrorForUuid(const std::string& uuid, const std::string& errorMessage) {
        errorMessagesForUuid[uuid] = errorMessage;
    }

    void sendResponseMessage(Message& message, const std::shared_ptr<TestWsServer::Connection>& connection) {
        auto msgData = message.getData();
        auto outMessage = std::make_shared<TestWsServer::OutMessage>(msgData->size());
        std::copy(msgData->begin(), msgData->end(), std::ostream_iterator<uint8_t>(*outMessage));
        connection->send(outMessage, nullptr, 130);
    }

    auto generateRandomData(size_t size) -> std::shared_ptr<std::vector<uint8_t>> {
        auto data = std::make_shared<std::vector<uint8_t>>(size);
        for (size_t i = 0; i < size; i++) {
            (*data)[i] = static_cast<uint8_t>(rand() % 256);
        }
        return data;
    }

    void writeUploadTestData(const std::string& data) {
        std::ofstream ofs(tempFile);
        ofs << data;
        ofs.close();
    }

    void writeUploadTestBinaryData(const std::vector<uint8_t>& data) {
        std::ofstream ofs(tempFile, std::ios::binary);
        ofs.write(reinterpret_cast<const char*>(data.data()), static_cast<std::streamsize>(data.size()));
        ofs.close();
    }

    auto readUploadedFile() -> std::string {
        if (!boost::filesystem::exists(targetFile)) {
            return "";
        }
        std::ifstream ifs(targetFile);
        std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
        return content;
    }

    auto readUploadedBinaryFile() -> std::vector<uint8_t> {
        if (!boost::filesystem::exists(targetFile)) {
            return {};
        }
        std::ifstream ifs(targetFile, std::ios::binary);
        std::vector<uint8_t> content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
        return content;
    }

    void configureUploadError(const std::string& uuid, const std::string& errorMessage) {
        if (uploadStates.find(uuid) != uploadStates.end()) {
            uploadStates[uuid].simulateError = true;
            uploadStates[uuid].errorMessage = errorMessage;
        }
    }

    void simulatePathValidationError(const std::string& uuid, const std::string& errorMessage) {
        // This will be used to send immediate error responses for validation failures
        auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
        errorMsg.push_string(errorMessage);
        sendResponseMessage(errorMsg, pWebsocketServerConnection);
    }

    void simulateJobValidationError(const std::string& uuid, const std::string& errorMessage) {
        // This will be used to send immediate error responses for job validation failures  
        auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
        errorMsg.push_string(errorMessage);
        sendResponseMessage(errorMsg, pWebsocketServerConnection);
    }
};

BOOST_FIXTURE_TEST_SUITE(file_upload_test_suite, FileUploadTestDataFixture)

BOOST_AUTO_TEST_CASE(test_file_upload_job_based_success) {
    auto uploadUuid = generateUUID();
    std::string testData = "This is test data for job-based file upload";
    
    // Set up test data for this UUID
    setTestDataForUuid(uploadUuid, testData);
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");  // Bundle hash (not used when jobId != 0)
    msg.push_string("uploaded_file.txt");
    msg.push_ulong(testData.size());
    msg.send();  // Send to client, not to server

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(200));

    // Verify the uploaded file
    BOOST_CHECK_EQUAL(readUploadedFile(), testData);
}

BOOST_AUTO_TEST_CASE(test_file_upload_bundle_based_success) {
    auto uploadUuid = generateUUID();
    std::string testData = "This is test data for bundle-based file upload";
    auto bundleHash = generateUUID();
    
    // Create bundle working directory configuration
    writeFileListNoJobWorkingDirectory(bundleHash, tempDir);
    
    // Set up test data for this UUID
    setTestDataForUuid(uploadUuid, testData);
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(0);  // No job ID, use bundle
    msg.push_string(bundleHash);  // Bundle hash for working directory resolution
    msg.push_string("bundle_uploaded_file.txt");
    msg.push_ulong(testData.size());
    msg.send();  // Send to client, not to server

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(200));

    // Verify the uploaded file in bundle directory
    std::string bundleUploadedFile = tempDir + "/bundle_uploaded_file.txt";
    BOOST_CHECK(boost::filesystem::exists(bundleUploadedFile));

    std::ifstream ifs(bundleUploadedFile);
    std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    BOOST_CHECK_EQUAL(content, testData);
}BOOST_AUTO_TEST_CASE(test_file_upload_invalid_path_outside_working_directory) {
    auto uploadUuid = generateUUID();
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("../outside_file.txt");  // Try to upload outside working directory
    msg.push_ulong(100);
    msg.send(pWebsocketServerConnection);

    // Wait for error response
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Target path for file upload is outside the working directory");
}

BOOST_AUTO_TEST_CASE(test_file_upload_invalid_job_id) {
    auto uploadUuid = generateUUID();
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(99999);  // Non-existent job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("test_file.txt");
    msg.push_ulong(100);
    msg.send(pWebsocketServerConnection);

    // Wait for error response
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job does not exist");
}

BOOST_AUTO_TEST_CASE(test_file_upload_job_submitting) {
    // Set job to submitting state
    database->operator()(update(jobTable).set(jobTable.submitting = 1).where(jobTable.id == jobId));
    
    auto uploadUuid = generateUUID();
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Job that's currently submitting
    msg.push_string("test_bundle_hash");
    msg.push_string("test_file.txt");
    msg.push_ulong(100);
    msg.send(pWebsocketServerConnection);

    // Wait for error response
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job is not submitted");
    
    // Reset job state
    database->operator()(update(jobTable).set(jobTable.submitting = 0).where(jobTable.id == jobId));
}

BOOST_AUTO_TEST_CASE(test_file_upload_large_file) {
    auto uploadUuid = generateUUID();
    // Generate 1MB of random data
    auto testData = generateRandomData(1024 * 1024);
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("large_file.bin");
    msg.push_ulong(testData->size());
    msg.send(pWebsocketServerConnection);

    // Wait for the upload process to start
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // Send data in chunks (simulate realistic upload)
    const size_t chunkSize = CHUNK_SIZE;
    size_t offset = 0;
    while (offset < testData->size()) {
        size_t currentChunkSize = std::min(chunkSize, testData->size() - offset);
        std::vector<uint8_t> chunk(testData->begin() + offset, testData->begin() + offset + currentChunkSize);
        
        auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg.push_bytes(chunk);
        sendResponseMessage(chunkMsg, pWebsocketServerConnection);
        
        offset += currentChunkSize;
        std::this_thread::sleep_for(std::chrono::milliseconds(1)); // Small delay between chunks
    }

    // Send completion message
    {
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uploadUuid);
        sendResponseMessage(completeMsg, pWebsocketServerConnection);
    }

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(500));

    // Verify the uploaded file
    auto uploadedData = readUploadedBinaryFile();
    BOOST_CHECK_EQUAL(uploadedData.size(), testData->size());
    BOOST_CHECK_EQUAL_COLLECTIONS(uploadedData.begin(), uploadedData.end(), testData->begin(), testData->end());
}

BOOST_AUTO_TEST_CASE(test_file_upload_zero_byte_file) {
    auto uploadUuid = generateUUID();
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("empty_file.txt");
    msg.push_ulong(0);  // Zero bytes
    msg.send(pWebsocketServerConnection);

    // Wait for the upload process to start
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // Send completion message immediately (no chunks for zero-byte file)
    {
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uploadUuid);
        sendResponseMessage(completeMsg, pWebsocketServerConnection);
    }

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(100));

    // Verify the uploaded file exists and is empty
    BOOST_CHECK(boost::filesystem::exists(tempDir + "/empty_file.txt"));
    BOOST_CHECK_EQUAL(boost::filesystem::file_size(tempDir + "/empty_file.txt"), 0);
}

BOOST_AUTO_TEST_CASE(test_file_upload_file_size_mismatch) {
    auto uploadUuid = generateUUID();
    std::string testData = "Short data";
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("mismatch_file.txt");
    msg.push_ulong(1000);  // Claim file is 1000 bytes, but actually send much less
    msg.send(pWebsocketServerConnection);

    // Wait for the upload process to start
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // Send small chunk
    {
        auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg.push_bytes(std::vector<uint8_t>(testData.begin(), testData.end()));
        sendResponseMessage(chunkMsg, pWebsocketServerConnection);
    }

    // Send completion message (this should detect size mismatch)
    {
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uploadUuid);
        sendResponseMessage(completeMsg, pWebsocketServerConnection);
    }

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(100));

    // Should get error response due to size mismatch
    bool foundErrorMessage = false;
    while (!receivedMessages.empty()) {
        auto message = receivedMessages.front();
        receivedMessages.pop();
        if (message->getId() == FILE_UPLOAD_ERROR) {
            auto errorMsg = message->pop_string();
            BOOST_CHECK(errorMsg.find("File size mismatch") != std::string::npos);
            foundErrorMessage = true;
            break;
        }
    }
    BOOST_CHECK(foundErrorMessage);
}

BOOST_AUTO_TEST_CASE(test_file_upload_nested_directory_creation) {
    auto uploadUuid = generateUUID();
    std::string testData = "Test data for nested directory";
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("subdir/nested/file.txt");  // This should create nested directories
    msg.push_ulong(testData.size());
    msg.send(pWebsocketServerConnection);

    // Wait for the upload process to start
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // Send file chunk
    {
        auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg.push_bytes(std::vector<uint8_t>(testData.begin(), testData.end()));
        sendResponseMessage(chunkMsg, pWebsocketServerConnection);
    }

    // Send completion message
    {
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uploadUuid);
        sendResponseMessage(completeMsg, pWebsocketServerConnection);
    }

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(100));

    // Verify the nested directories were created and file was uploaded
    std::string nestedFile = tempDir + "/subdir/nested/file.txt";
    BOOST_CHECK(boost::filesystem::exists(nestedFile));
    
    std::ifstream ifs(nestedFile);
    std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    BOOST_CHECK_EQUAL(content, testData);
}

BOOST_AUTO_TEST_CASE(test_file_upload_write_permission_error) {
    // This test simulates a write permission error
    auto uploadUuid = generateUUID();
    
    // Try to write to a read-only directory (this will be detected during directory creation)
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("/read_only/file.txt");  // This should fail path validation
    msg.push_ulong(100);
    msg.send(pWebsocketServerConnection);

    // Wait for error response
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    // Should get an error about invalid path
}

BOOST_AUTO_TEST_CASE(test_file_upload_multiple_chunks) {
    auto uploadUuid = generateUUID();
    std::string part1 = "First part of the file. ";
    std::string part2 = "Second part of the file. ";
    std::string part3 = "Third and final part.";
    std::string completeData = part1 + part2 + part3;
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("multi_chunk_file.txt");
    msg.push_ulong(completeData.size());
    msg.send(pWebsocketServerConnection);

    // Wait for the upload process to start
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // Send multiple chunks
    {
        auto chunkMsg1 = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg1.push_bytes(std::vector<uint8_t>(part1.begin(), part1.end()));
        sendResponseMessage(chunkMsg1, pWebsocketServerConnection);
        
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
        
        auto chunkMsg2 = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg2.push_bytes(std::vector<uint8_t>(part2.begin(), part2.end()));
        sendResponseMessage(chunkMsg2, pWebsocketServerConnection);
        
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
        
        auto chunkMsg3 = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg3.push_bytes(std::vector<uint8_t>(part3.begin(), part3.end()));
        sendResponseMessage(chunkMsg3, pWebsocketServerConnection);
    }

    // Send completion message
    {
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uploadUuid);
        sendResponseMessage(completeMsg, pWebsocketServerConnection);
    }

    // Wait for processing
    std::this_thread::sleep_for(std::chrono::milliseconds(100));

    // Verify the complete file was assembled correctly
    std::string uploadedFile = tempDir + "/multi_chunk_file.txt";
    BOOST_CHECK(boost::filesystem::exists(uploadedFile));
    
    std::ifstream ifs(uploadedFile);
    std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    BOOST_CHECK_EQUAL(content, completeData);
}

BOOST_AUTO_TEST_CASE(test_file_upload_partial_file_cleanup_on_error) {
    auto uploadUuid = generateUUID();
    std::string partialData = "This is partial file data";
    
    Message msg(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msg.push_string(uploadUuid);
    msg.push_uint(1234);  // Use existing job ID
    msg.push_string("test_bundle_hash");
    msg.push_string("partial_cleanup_file.txt");
    msg.push_ulong(1000);  // Expect 1000 bytes
    msg.send(pWebsocketServerConnection);

    // Wait for the upload process to start
    while (receivedMessages.empty()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }

    // Send only partial data (much less than expected 1000 bytes)
    {
        auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uploadUuid);
        chunkMsg.push_bytes(std::vector<uint8_t>(partialData.begin(), partialData.end()));
        sendResponseMessage(chunkMsg, pWebsocketServerConnection);
    }

    // Verify partial file exists before completion
    std::string partialFile = tempDir + "/partial_cleanup_file.txt";
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    BOOST_CHECK(boost::filesystem::exists(partialFile));

    // Send completion message (this should trigger size mismatch error and cleanup)
    {
        auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uploadUuid);
        sendResponseMessage(completeMsg, pWebsocketServerConnection);
    }

    // Wait for processing and cleanup
    std::this_thread::sleep_for(std::chrono::milliseconds(200));

    // Verify partial file has been cleaned up after error
    BOOST_CHECK(!boost::filesystem::exists(partialFile));
    
    // Verify we received an error message
    bool foundErrorMessage = false;
    while (!receivedMessages.empty()) {
        auto message = receivedMessages.front();
        receivedMessages.pop();
        if (message->getId() == FILE_UPLOAD_ERROR) {
            auto errorMsg = message->pop_string();
            BOOST_CHECK(errorMsg.find("File size mismatch") != std::string::npos);
            foundErrorMessage = true;
            break;
        }
    }
    BOOST_CHECK(foundErrorMessage);
}

BOOST_AUTO_TEST_SUITE_END()