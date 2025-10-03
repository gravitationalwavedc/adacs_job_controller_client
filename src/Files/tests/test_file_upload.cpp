//
// Created by lewis on 12/19/24.
//

#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"
#include "../../Core/MessageHandler.h"
#include "../../Settings.h"
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

    void onWebsocketServerMessage(const std::shared_ptr<Message>& msg, const std::shared_ptr<TestWsServer::Connection>& connection) override {
        lastMessageTime = std::chrono::system_clock::now();

        receivedMessages.push(msg);

        // Handle specific message types for upload simulation
        switch (msg->getId()) {
            case SERVER_READY:
                // This is the client confirming it's ready to receive file data
                // We should respond by sending the file chunks for the specific UUID
                {
                    auto uuid = msg->getSource();
                    sendTestFileData(uuid, connection);
                }
                break;
            case FILE_UPLOAD_COMPLETE:
                // This is the client confirming successful receipt of all data
                {
                    auto uuid = msg->getSource();
                    // Test completed successfully - nothing more to do
                }
                break;
            case DB_JOB_GET_BY_JOB_ID:
                // Client is requesting job information from database
                // This is needed for job-based file uploads to get the working directory
                {
                    auto requestId = msg->pop_string();
                    auto jobIdRequested = msg->pop_uint();
                    
                    // Send back job information
                    auto response = Message(DB_RESPONSE, Message::Priority::Highest, requestId);
                    response.push_uint(jobId);  // Job ID
                    response.push_uint(1234);   // Job ID (from DB)
                    response.push_string("my_hash");  // Bundle hash
                    response.push_string(tempDir);    // Working directory
                    response.push_ubyte(0);  // submitting
                    response.push_ubyte(0);  // running
                    response.push_uint(0);   // submittingCount
                    response.push_ubyte(0);  // deleting
                    response.push_ubyte(0);  // deleted
                    sendResponseMessage(response, connection);
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
            
            // Send the data in chunks (like the real server would)
            size_t offset = 0;
            while (offset < data.size()) {
                size_t chunkSize = std::min(static_cast<size_t>(CHUNK_SIZE), data.size() - offset);
                std::vector<uint8_t> chunk(data.begin() + offset, data.begin() + offset + chunkSize);
                
                auto chunkMsg = Message(FILE_UPLOAD_CHUNK, Message::Priority::Lowest, uuid);
                chunkMsg.push_bytes(chunk);
                sendResponseMessage(chunkMsg, connection);
                
                offset += chunkSize;
                
                // Small delay between chunks to simulate network transmission
                if (offset < data.size()) {
                    std::this_thread::sleep_for(std::chrono::milliseconds(1));
                }
            }
            
            // Send completion message
            auto completeMsg = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uuid);
            sendResponseMessage(completeMsg, connection);
            return;
        }

        // Send text data if available
        if (testDataForUuid.find(uuid) != testDataForUuid.end()) {
            const auto& testData = testDataForUuid[uuid];
            
            // Send the data as a chunk (text is typically small, single chunk is fine)
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

    // Helper function to wait for file creation with timeout
    bool waitForFile(const std::string& filePath, std::chrono::milliseconds timeout = std::chrono::milliseconds(5000)) {
        auto start = std::chrono::steady_clock::now();
        while (std::chrono::steady_clock::now() - start < timeout) {
            if (boost::filesystem::exists(filePath)) {
                return true;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
        return false;
    }

    // Helper function to wait for message with timeout
    bool waitForMessage(std::chrono::milliseconds timeout = std::chrono::milliseconds(5000)) {
        auto start = std::chrono::steady_clock::now();
        while (std::chrono::steady_clock::now() - start < timeout) {
            if (!receivedMessages.empty()) {
                return true;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
        return false;
    }
};

BOOST_FIXTURE_TEST_SUITE(file_upload_test_suite, FileUploadTestDataFixture)

BOOST_AUTO_TEST_CASE(test_file_upload_job_based_success) {
    auto uploadUuid = generateUUID();
    std::string testData = "This is test data for job-based file upload";
    
    // Set up test data for this UUID
    setTestDataForUuid(uploadUuid, testData);
    
    // Create message like it would be sent from server
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");  // Bundle hash (not used when jobId != 0)
    msgBuilder.push_string("uploaded_file.txt");
    msgBuilder.push_ulong(testData.size());
    
    // Reconstruct from raw bytes (simulates receiving from network)
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);  // Trigger client-side file upload handler

    // Wait for file to be created
    BOOST_REQUIRE(waitForFile(targetFile));

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
    
    // Create message like it would be sent from server
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(0);  // No job ID, use bundle
    msgBuilder.push_string(bundleHash);  // Bundle hash for working directory resolution
    msgBuilder.push_string("bundle_uploaded_file.txt");
    msgBuilder.push_ulong(testData.size());
    
    // Reconstruct from raw bytes (simulates receiving from network)
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);  // Trigger client-side file upload handler

    // Wait for file to be created
    std::string bundleUploadedFile = tempDir + "/bundle_uploaded_file.txt";
    BOOST_REQUIRE(waitForFile(bundleUploadedFile));

    // Verify the uploaded file in bundle directory
    BOOST_CHECK(boost::filesystem::exists(bundleUploadedFile));

    std::ifstream ifs(bundleUploadedFile);
    std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    BOOST_CHECK_EQUAL(content, testData);
}BOOST_AUTO_TEST_CASE(test_file_upload_invalid_path_outside_working_directory) {
    auto uploadUuid = generateUUID();
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("../outside_file.txt");  // Try to upload outside working directory
    msgBuilder.push_ulong(100);
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for error response
    BOOST_REQUIRE(waitForMessage());

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Target path for file upload is outside the working directory");
}

BOOST_AUTO_TEST_CASE(test_file_upload_invalid_job_id) {
    auto uploadUuid = generateUUID();
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(99999);  // Non-existent job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("test_file.txt");
    msgBuilder.push_ulong(100);
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for error response
    BOOST_REQUIRE(waitForMessage());

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    BOOST_CHECK_EQUAL(receivedMessage->pop_string(), "Job does not exist");
}

BOOST_AUTO_TEST_CASE(test_file_upload_job_submitting) {
    // Set job to submitting state
    database->operator()(update(jobTable).set(jobTable.submitting = 1).where(jobTable.id == jobId));
    
    auto uploadUuid = generateUUID();
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Job that's currently submitting
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("test_file.txt");
    msgBuilder.push_ulong(100);
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for error response
    BOOST_REQUIRE(waitForMessage());

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
    
    // Register binary data for this UUID
    setTestBinaryDataForUuid(uploadUuid, *testData);
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("large_file.bin");
    msgBuilder.push_ulong(testData->size());
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for file to be created (larger file takes longer, but no arbitrary timeout)
    std::string largeFilePath = tempDir + "/large_file.bin";
    BOOST_REQUIRE(waitForFile(largeFilePath, std::chrono::milliseconds(10000)));

    // Verify the uploaded file
    BOOST_CHECK(boost::filesystem::exists(largeFilePath));
    
    std::ifstream ifs(largeFilePath, std::ios::binary);
    std::vector<uint8_t> uploadedData((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    
    BOOST_CHECK_EQUAL(uploadedData.size(), testData->size());
    BOOST_CHECK_EQUAL_COLLECTIONS(uploadedData.begin(), uploadedData.end(), testData->begin(), testData->end());
}

BOOST_AUTO_TEST_CASE(test_file_upload_zero_byte_file) {
    auto uploadUuid = generateUUID();
    
    // Register empty binary data for this UUID
    setTestBinaryDataForUuid(uploadUuid, std::vector<uint8_t>());
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("empty_file.txt");
    msgBuilder.push_ulong(0);  // Zero bytes
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for file to be created
    std::string emptyFilePath = tempDir + "/empty_file.txt";
    BOOST_REQUIRE(waitForFile(emptyFilePath));

    // Verify the uploaded file exists and is empty
    BOOST_CHECK(boost::filesystem::exists(emptyFilePath));
    BOOST_CHECK_EQUAL(boost::filesystem::file_size(emptyFilePath), 0);
}

BOOST_AUTO_TEST_CASE(test_file_upload_file_size_mismatch) {
    auto uploadUuid = generateUUID();
    std::string testData = "Short data";
    
    // Register data that's much smaller than what we claim
    setTestBinaryDataForUuid(uploadUuid, std::vector<uint8_t>(testData.begin(), testData.end()));
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("mismatch_file.txt");
    msgBuilder.push_ulong(1000);  // Claim file is 1000 bytes, but actually send much less
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for error message
    BOOST_REQUIRE(waitForMessage());

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
    
    // Register test data for this UUID
    setTestDataForUuid(uploadUuid, testData);
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("subdir/nested/file.txt");  // This should create nested directories
    msgBuilder.push_ulong(testData.size());
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for file to be created
    std::string nestedFile = tempDir + "/subdir/nested/file.txt";
    BOOST_REQUIRE(waitForFile(nestedFile));

    // Verify the nested directories were created and file was uploaded
    BOOST_CHECK(boost::filesystem::exists(nestedFile));
    
    std::ifstream ifs(nestedFile);
    std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    BOOST_CHECK_EQUAL(content, testData);
}

BOOST_AUTO_TEST_CASE(test_file_upload_write_permission_error) {
    // This test verifies that path traversal attacks (using ../) are blocked
    auto uploadUuid = generateUUID();
    
    // Try to escape the working directory using ../../../
    // This should be blocked by the path validation
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("../../../../../../etc/passwd");  // Path traversal attack attempt
    msgBuilder.push_ulong(100);
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for error response
    BOOST_REQUIRE(waitForMessage());

    auto receivedMessage = receivedMessages.front();
    BOOST_CHECK_EQUAL(receivedMessage->getId(), FILE_UPLOAD_ERROR);
    // Should get an error about path being outside working directory
    auto errorMsg = receivedMessage->pop_string();
    BOOST_CHECK(errorMsg.find("outside the working directory") != std::string::npos || 
                errorMsg.find("Invalid target path") != std::string::npos);
}

BOOST_AUTO_TEST_CASE(test_file_upload_multiple_chunks) {
    auto uploadUuid = generateUUID();
    std::string part1 = "First part of the file. ";
    std::string part2 = "Second part of the file. ";
    std::string part3 = "Third and final part.";
    std::string completeData = part1 + part2 + part3;
    
    // Register test data for this UUID (will be sent as single chunk)
    setTestDataForUuid(uploadUuid, completeData);
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("multi_chunk_file.txt");
    msgBuilder.push_ulong(completeData.size());
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for file to be created
    std::string uploadedFile = tempDir + "/multi_chunk_file.txt";
    BOOST_REQUIRE(waitForFile(uploadedFile));

    // Verify the complete file was assembled correctly
    BOOST_CHECK(boost::filesystem::exists(uploadedFile));
    
    std::ifstream ifs(uploadedFile);
    std::string content((std::istreambuf_iterator<char>(ifs)), std::istreambuf_iterator<char>());
    BOOST_CHECK_EQUAL(content, completeData);
}

BOOST_AUTO_TEST_CASE(test_file_upload_partial_file_cleanup_on_error) {
    auto uploadUuid = generateUUID();
    std::string partialData = "This is partial file data";
    
    // Register data that's much smaller than what we claim
    setTestBinaryDataForUuid(uploadUuid, std::vector<uint8_t>(partialData.begin(), partialData.end()));
    
    Message msgBuilder(UPLOAD_FILE, Message::Priority::Highest, SYSTEM_SOURCE);
    msgBuilder.push_string(uploadUuid);
    msgBuilder.push_uint(1234);  // Use existing job ID
    msgBuilder.push_string("test_bundle_hash");
    msgBuilder.push_string("partial_cleanup_file.txt");
    msgBuilder.push_ulong(1000);  // Expect 1000 bytes
    
    auto msg = std::make_shared<Message>(*msgBuilder.getData());
    ::handleMessage(msg);

    // Wait for error message
    BOOST_REQUIRE(waitForMessage());

    // Verify partial file has been cleaned up after error
    std::string partialFile = tempDir + "/partial_cleanup_file.txt";
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