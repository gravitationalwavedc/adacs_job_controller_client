//
// Created by lewis on 12/19/24.
//
// File Upload Implementation for ADACS Job Controller Client
//
// This module handles file upload operations from the server to the client's working directory.
// The file upload process uses a WebSocket-based protocol with chunked data transfer and
// comprehensive error handling.
//
// PROTOCOL OVERVIEW:
// ==================
// 1. Client receives UPLOAD_FILE message containing:
//    - UUID: Unique identifier for this upload session
//    - Target Path: Relative path where file should be written (within working directory)
//    - File Size: Expected total size in bytes for validation
//
// 2. Client establishes WebSocket connection using UUID as authentication token
//
// 3. Handshake sequence:
//    - Server sends SERVER_READY when connection is authenticated
//    - Client responds with SERVER_READY to confirm readiness to receive data
//
// 4. Data transfer:
//    - Server sends FILE_UPLOAD_CHUNK messages containing binary file data
//    - Client writes each chunk to the target file
//    - Multiple chunks are assembled sequentially to recreate the complete file
//
// 5. Completion:
//    - Server sends FILE_UPLOAD_COMPLETE when all data has been sent
//    - Client validates final file size matches expected size
//    - Client responds with FILE_UPLOAD_COMPLETE to confirm successful receipt
//
// ERROR HANDLING:
// ===============
// - Path validation: Ensures target path is within working directory (security)
// - Directory creation: Creates parent directories as needed
// - File permissions: Validates write access to target location
// - Size validation: Confirms final file size matches expected size
// - Connection errors: Handles WebSocket disconnections and timeouts
// - All errors result in FILE_UPLOAD_ERROR messages being sent to server
//
// DESIGN CHOICES:
// ===============
// - Uses shared_ptr for file streams to ensure proper cleanup in lambda captures
// - Validates paths using boost::filesystem for cross-platform compatibility
// - Creates parent directories automatically for nested file paths
// - Opens files in binary mode to handle all file types correctly
// - Flushes after each chunk to ensure data persistence in case of interruption
// - Uses detached threads to prevent blocking the main message loop
//

#include "../Bundle/BundleManager.h"
#include "../DB/sJob.h"
#include "../Settings.h"
#include "FileHandling.h"
#include "glog/logging.h"
#include <boost/filesystem.hpp>
#include <cstdint>
#include <fstream>
#include <future>

/**
 * Sends a message over the WebSocket connection during file upload operations.
 * 
 * This is a utility function that handles the low-level details of sending messages
 * over the WebSocket connection. It converts the Message object to the appropriate
 * binary format expected by the Simple-WebSocket-Server library.
 * 
 * @param message The message to send (will be serialized to binary)
 * @param pConnection The WebSocket connection to send over
 * @param callback Optional callback function to execute when send completes
 * 
 * @throws std::runtime_error if the connection has been closed
 * 
 * Note: The magic number 130 is the WebSocket opcode for binary frames
 */
void sendUploadMessage(Message& message, const std::shared_ptr<WsClient::Connection>& pConnection, const std::function<void()>& callback = [] {}) {
    auto msgData = message.getData();
    auto outMessage = std::make_shared<WsClient::OutMessage>(msgData->size());
    std::copy(msgData->begin(), msgData->end(), std::ostream_iterator<uint8_t>(*outMessage));

    if (!pConnection) {
        throw std::runtime_error("File upload connection was closed.");
    }

    pConnection->send(
            outMessage,
            [&, callback](const SimpleWeb::error_code &/*errorCode*/) {
                callback();
            },
            // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
            130  // WebSocket binary frame opcode
    );
}

/**
 * Core implementation of file upload handling.
 * 
 * This function processes an UPLOAD_FILE message and manages the complete file upload
 * workflow including WebSocket connection establishment, path validation, file writing,
 * and error handling. The function runs in its own thread to avoid blocking the main
 * message processing loop.
 * 
 * ALGORITHM:
 * 1. Extract upload parameters (UUID, target path, expected file size)
 * 2. Validate and sanitize the target file path
 * 3. Establish WebSocket connection using UUID as authentication token
 * 4. Set up message handlers for the upload protocol
 * 5. Wait for server to send file data in chunks
 * 6. Write chunks to target file and validate final size
 * 7. Send confirmation or error messages as appropriate
 * 
 * SECURITY CONSIDERATIONS:
 * - All target paths are validated to be within the working directory
 * - Path traversal attacks (../) are prevented by path canonicalization
 * - File size limits are enforced through size validation
 * - Parent directories are created safely without following symlinks
 * 
 * ERROR RECOVERY:
 * - Connection failures result in FILE_UPLOAD_ERROR messages
 * - Write failures clean up partial files and report errors
 * - Size mismatches are detected and reported after completion
 * - All exceptions are caught and converted to error messages
 * 
 * @param msg The UPLOAD_FILE message containing upload parameters
 */
void handleFileUploadImpl(const std::shared_ptr<Message> &msg) { // NOLINT(readability-function-cognitive-complexity)
    // Extract upload parameters from the message
    // Message format: [UUID][Job ID][Bundle Hash][Target Path][File Size]
    auto uuid = msg->pop_string();        // Unique identifier for this upload session
    auto jobId = msg->pop_uint();         // Job ID (0 if using bundle-based upload)
    auto bundleHash = msg->pop_string();  // Bundle hash for bundle-based working directory resolution
    auto targetPath = msg->pop_string();  // Relative path where file should be written
    auto fileSize = msg->pop_ulong();     // Expected total file size for validation

    // Connection and file handling objects
    std::shared_ptr<WsClient> client;                    // WebSocket client for server communication
    std::shared_ptr<WsClient::Connection> pConnection = nullptr;  // Active WebSocket connection
    std::shared_ptr<std::ofstream> outputFile = nullptr;         // Output file stream (shared_ptr for lambda capture safety)

    auto config = readClientConfig();

    // Configure WebSocket connection
    // The UUID serves as both session identifier and authentication token
    auto url = std::string{config["websocketEndpoint"]} + "?token=" + uuid;
    
#ifndef BUILD_TESTS
    // Production builds use SSL/TLS configuration from config file
    bool insecure = false;
    if (config.contains("insecure")) {
        insecure = static_cast<bool>(config["insecure"]);
    }

    client = std::make_shared<WsClient>(url, !insecure);
#else
    // Test builds always use insecure connections for simplicity
    client = std::make_shared<WsClient>(url);
#endif

    std::string workingDirectory;
    
    // WORKING DIRECTORY RESOLUTION
    // ============================
    // Follow the same pattern as FileDownload.cpp to determine working directory
    // Two cases: job-based (jobId != 0) or bundle-based (jobId == 0)
    
    if (jobId != 0) {
        // CASE 1: Job-based upload - look up working directory from job record
        sJob job;
        try {
            job = sJob::getOrCreateByJobId(jobId);
            if (job.id == 0) {
                throw std::runtime_error("Job did not exist");
            }
        } catch (std::runtime_error&) {
            LOG(ERROR) << "Job does not exist with ID " << jobId;
            // Report that the job doesn't exist
            auto result = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string("Job does not exist");
            result.send();
            return;
        }

        if (static_cast<bool>(job.submitting)) {
            LOG(INFO) << "Job " << jobId << " is submitting, cannot upload files";
            // Report that the job hasn't been submitted yet
            auto result = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string("Job is not submitted");
            result.send();
            return;
        }

        // Get the working directory from the job record
        workingDirectory = job.workingDirectory;
        LOG(INFO) << "Using job-based working directory: " << workingDirectory << " for job " << jobId;
    } else {
        // CASE 2: Bundle-based upload - use bundle manager to determine working directory
        try {
            auto bundlePath = getBundlePath();
            workingDirectory = BundleManager::Singleton()->runBundle_string("working_directory", bundleHash, targetPath, "file_upload");
            LOG(INFO) << "Using bundle-based working directory: " << workingDirectory << " for bundle " << bundleHash;
        } catch (std::exception& error) {
            LOG(ERROR) << "Failed to get working directory from bundle " << bundleHash << ": " << error.what();
            auto result = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string("Failed to determine working directory from bundle");
            result.send();
            return;
        }
    }

    // PATH VALIDATION AND SANITIZATION
    // ================================
    
    // Remove leading slashes to ensure relative path interpretation
    // This prevents absolute path attacks and ensures paths are relative to working directory
    while (!targetPath.empty() && targetPath[0] == '/') {
        targetPath = targetPath.substr(1);
    }

    // Resolve target path and validate security constraints
    try {
        // Construct the full path to the target file
        auto fullTargetPath = boost::filesystem::path(workingDirectory) / targetPath;

        // Create parent directories if they don't exist
        // This allows uploading files to nested directory structures
        boost::filesystem::create_directories(fullTargetPath.parent_path());

        // Canonicalize the working directory and the target parent path to resolve
        // symlinks and any . or .. components. This prevents directory traversal
        // attacks, including cases where a symlink inside the working directory
        // points outside but happens to share a string prefix with the working dir.
        auto canonicalWorking = boost::filesystem::canonical(boost::filesystem::path(workingDirectory));
        auto canonicalTargetParent = boost::filesystem::canonical(fullTargetPath.parent_path());

        // Reconstruct full canonical target path
        auto canonicalTargetPath = canonicalTargetParent / fullTargetPath.filename();
        targetPath = canonicalTargetPath.string();
    } catch (boost::filesystem::filesystem_error &error) {
        LOG(WARNING) << "Invalid target path for file upload " << targetPath << ": " << error.what();
        // Send error response and abort upload
        auto result = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Invalid target path for file upload");
        result.send();
        return;
    }

    // SECURITY CHECK: Ensure canonical target path is within the canonical working directory
    // Walk up from the canonical target path and ensure we eventually reach the canonical working dir
    bool isWithin = false;
    try {
        auto canonicalWorking = boost::filesystem::canonical(boost::filesystem::path(workingDirectory));
        auto canonicalTarget = boost::filesystem::path(targetPath);
        auto current = canonicalTarget;
        while (true) {
            if (current == canonicalWorking) {
                isWithin = true;
                break;
            }
            if (current == current.root_path()) break;
            current = current.parent_path();
        }
    } catch (boost::filesystem::filesystem_error &error) {
        LOG(WARNING) << "Failed during containment check for path " << targetPath << ": " << error.what();
    }

    if (!isWithin) {
        LOG(WARNING) << "Target path for file upload is outside the working directory " << targetPath << " (working dir: " << workingDirectory << ")";
        auto result = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Target path for file upload is outside the working directory");
        result.send();
        return;
    }

    LOG(INFO) << "Starting file upload to " << targetPath << " (" << fileSize << " bytes)" 
              << " [UUID: " << uuid << ", JobID: " << jobId << ", Bundle: " << bundleHash << "]";

    // CLEANUP HELPER FUNCTIONS
    // ========================
    // These lambdas ensure proper cleanup of resources in all exit paths
    
    // Clean up partial files on error - important for security and storage management
    auto cleanupPartialFile = [&targetPath, &outputFile]() {
        try {
            // Close file first if it's open
            if (outputFile && outputFile->is_open()) {
                outputFile->close();
            }
            
            // Remove the partial file if it exists
            if (boost::filesystem::exists(targetPath)) {
                boost::filesystem::remove(targetPath);
                LOG(INFO) << "Cleaned up partial file: " << targetPath;
            }
        } catch (std::exception& e) {
            LOG(WARNING) << "Failed to clean up partial file " << targetPath << ": " << e.what();
            // Don't throw - cleanup failures shouldn't prevent error reporting
        }
    };
    
    // Normal connection cleanup for successful uploads
    auto closeConnection = [&client, &pConnection, &outputFile]() {
        try {
            // Close output file stream if open
            if (outputFile && outputFile->is_open()) {
                outputFile->close();
            }
        } catch (std::exception&) {
            // File close errors are logged but don't prevent connection cleanup
        }
        try {
            // Send WebSocket close frame with normal closure status
            if (pConnection) {
                pConnection->send_close(1000);  // 1000 = normal closure
            }
        } catch (std::exception&) {
            // Connection may already be closed
        }
        try {
            // Stop the WebSocket client
            if (client) {
                client->stop();
            }
        } catch (std::exception&) {
            // Client may already be stopped
        }
    };
    
    // Error cleanup - clean up partial files AND close connections
    auto closeConnectionWithCleanup = [&cleanupPartialFile, &closeConnection]() {
        cleanupPartialFile();
        closeConnection();
    };

    // WEBSOCKET MESSAGE HANDLERS
    // ==========================
    // These handlers implement the file upload protocol state machine
    
    client->on_message = [&](std::shared_ptr<WsClient::Connection> connection, std::shared_ptr<WsClient::InMessage> in_message) {
        // Convert incoming message to our Message format
        auto data = std::string{std::istreambuf_iterator<char>(*in_message), std::istreambuf_iterator<char>()};
        auto message = Message(std::vector<uint8_t>(data.begin(), data.end()));
        
        // Process message based on protocol state
        switch (message.getId()) {
            case SERVER_READY:
                // HANDSHAKE: Server confirms connection is authenticated and ready
                // We must respond with SERVER_READY to confirm we're ready to receive data
                // This two-way handshake ensures both sides are prepared before data transfer begins
                {
                    LOG(INFO) << "Server ready for file upload: " << uuid;
                    auto response = Message(SERVER_READY, Message::Priority::Highest, uuid);
                    sendUploadMessage(response, connection);
                }
                break;
            case FILE_UPLOAD_CHUNK:
                // DATA TRANSFER: Receive and write file chunk
                // Chunks arrive in sequence and must be written in order to reconstruct the file
                {
                    auto chunkData = message.pop_bytes();
                    
                    // Lazy file opening: open the target file on first chunk
                    // This ensures we don't create empty files if no data arrives
                    if (!outputFile) {
                        outputFile = std::make_shared<std::ofstream>(targetPath, std::ios::binary | std::ios::trunc);
                        if (!outputFile->is_open()) {
                            LOG(ERROR) << "Failed to open target file for writing: " << targetPath;
                            auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
                            errorMsg.push_string("Failed to open target file for writing");
                            // Wait for the message to be sent before closing/cleaning up the connection
                            sendUploadMessage(errorMsg, connection, [closeConnectionWithCleanup]() { closeConnectionWithCleanup(); });
                            return;
                        }
                        LOG(INFO) << "Opened target file for writing: " << targetPath;
                    }
                    
                    // Write chunk data to file
                    try {
                        outputFile->write(reinterpret_cast<const char*>(chunkData.data()), static_cast<std::streamsize>(chunkData.size()));
                        outputFile->flush();  // Ensure data is written to disk immediately for safety
                    } catch (std::exception& e) {
                        LOG(ERROR) << "Failed to write chunk to file: " << e.what();
                        auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
                        errorMsg.push_string("Failed to write chunk to file");
                        // Ensure the error message is delivered before cleanup
                        sendUploadMessage(errorMsg, connection, [closeConnectionWithCleanup]() { closeConnectionWithCleanup(); });
                        return;
                    }
                }
                break;
            case FILE_UPLOAD_COMPLETE:
                // COMPLETION: Server has sent all data, validate and confirm receipt
                // This is the critical validation step that ensures data integrity
                {
                    // For zero-byte files, we may not have opened the file yet (no chunks sent)
                    // Create an empty file in this case
                    if (!outputFile && fileSize == 0) {
                        outputFile = std::make_shared<std::ofstream>(targetPath, std::ios::binary | std::ios::trunc);
                        if (!outputFile->is_open()) {
                            LOG(ERROR) << "Failed to create zero-byte file: " << targetPath;
                            auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
                            errorMsg.push_string("Failed to create zero-byte file");
                            sendUploadMessage(errorMsg, connection);
                            closeConnectionWithCleanup();
                            return;
                        }
                        LOG(INFO) << "Created zero-byte file: " << targetPath;
                    }
                    
                    // Close the output file to ensure all data is flushed
                    try {
                        if (outputFile && outputFile->is_open()) {
                            outputFile->close();
                            LOG(INFO) << "Closed output file: " << targetPath;
                        }
                    } catch (std::exception& e) {
                        LOG(WARNING) << "Error closing output file: " << e.what();
                    }
                    
                    // INTEGRITY CHECK: Verify final file size matches expected size
                    // This catches truncation, corruption, or incomplete transfers
                    if (boost::filesystem::exists(targetPath)) {
                        auto actualSize = boost::filesystem::file_size(targetPath);
                        if (actualSize != fileSize) {
                            LOG(ERROR) << "File size mismatch for " << targetPath 
                                      << ": expected " << fileSize 
                                      << " bytes, got " << actualSize << " bytes";
                            auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
                            errorMsg.push_string("File size mismatch: expected " + std::to_string(fileSize) + ", got " + std::to_string(actualSize));
                            // Send error and clean up only after the message has been sent
                            sendUploadMessage(errorMsg, connection, [closeConnectionWithCleanup]() { closeConnectionWithCleanup(); });
                            return;
                        }
                        LOG(INFO) << "File upload completed successfully: " << targetPath 
                                 << " (" << actualSize << " bytes)";
                    } else {
                        // File doesn't exist - this shouldn't happen if chunks were written correctly
                        LOG(ERROR) << "Uploaded file does not exist: " << targetPath;
                        auto errorMsg = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
                        errorMsg.push_string("Uploaded file does not exist after completion");
                        sendUploadMessage(errorMsg, connection, [closeConnectionWithCleanup]() { closeConnectionWithCleanup(); });
                        return;
                    }
                    
                    // Send completion confirmation back to server and close when delivered
                    auto response = Message(FILE_UPLOAD_COMPLETE, Message::Priority::Highest, uuid);
                    sendUploadMessage(response, connection, [closeConnection]() { closeConnection(); });
                }
                break;
            default:
                // Unexpected message type - log warning but don't crash
                // This provides resilience against protocol changes or malformed messages
                LOG(WARNING) << "Unknown message ID received during file upload: " << message.getId() << " for " << uuid;
                break;
        }
    };

    // CONNECTION EVENT HANDLERS
    // =========================
    
    client->on_open = [&](std::shared_ptr<WsClient::Connection> connection) {
        pConnection = connection;
        LOG(INFO) << "File upload WebSocket connection opened for " << uuid;
    };

    client->on_close = [&](std::shared_ptr<WsClient::Connection> /*connection*/, int status, const std::string & /*reason*/) {
        LOG(INFO) << "File upload WebSocket connection closed with status " << status << " for " << uuid;
        pConnection = nullptr;
    };

    client->on_error = [&](std::shared_ptr<WsClient::Connection> /*connection*/, const SimpleWeb::error_code &ec) {
        LOG(ERROR) << "File upload WebSocket error: " << ec.message() << " for " << uuid;
        closeConnectionWithCleanup();  // Clean up partial files on connection errors
    };

    // START THE WEBSOCKET CONNECTION
    // ==============================
    // This initiates the connection and begins the upload protocol
    try {
        LOG(INFO) << "Connecting to server for file upload: " << url;
        client->start();  // This call blocks until the connection closes
    } catch (std::exception &error) {
        LOG(ERROR) << "Error establishing file upload connection for " << uuid << ": " << error.what();
        // Report connection error back to the server
        auto result = Message(FILE_UPLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Connection error during file upload");
        result.send();
    }
}

/**
 * Main entry point for file upload handling.
 * 
 * This function receives UPLOAD_FILE messages and delegates the actual work
 * to handleFileUploadImpl() running in a separate thread. This design prevents
 * file upload operations from blocking the main message processing loop.
 * 
 * THREADING DESIGN:
 * - Main thread continues processing other messages
 * - Upload thread handles WebSocket connection and file I/O
 * - Thread is detached so it can complete independently
 * - Each upload gets its own thread for parallel processing
 * 
 * @param msg The UPLOAD_FILE message to process
 */
void handleFileUpload(const std::shared_ptr<Message> &msg) {
    // Spawn detached thread to handle upload without blocking main message loop
    auto thread = std::thread{[msg] { handleFileUploadImpl(msg); }};
    thread.detach();
}