//
// Created by lewis on 9/28/22.
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

void sendMessage(Message& message, const std::shared_ptr<WsClient::Connection>& pConnection, const std::function<void()>& callback = [] {}) {
    auto msgData = message.getData();
    auto outMessage = std::make_shared<WsClient::OutMessage>(msgData->size());
    std::copy(msgData->begin(), msgData->end(), std::ostream_iterator<uint8_t>(*outMessage));

    if (!pConnection) {
        throw std::runtime_error("File transfer connection was closed.");
    }

    pConnection->send(
            outMessage,
            [&, callback](const SimpleWeb::error_code &/*errorCode*/) {
                callback();
            },
            // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
            130
    );
}

void handleFileDownloadImpl(const std::shared_ptr<Message> &msg) { // NOLINT(readability-function-cognitive-complexity)
    // Get the job details
    auto jobId = msg->pop_uint();
    auto uuid = msg->pop_string();
    auto bundleHash = msg->pop_string();
    auto filePath = msg->pop_string();

    std::shared_ptr<WsClient> client;
    std::shared_ptr<WsClient::Connection> pConnection = nullptr;

    auto config = readClientConfig();

    auto url = std::string{config["websocketEndpoint"]} + "?token=" + uuid;
#ifndef BUILD_TESTS
    bool insecure = false;
    if (config.contains("insecure")) {
        insecure = static_cast<bool>(config["insecure"]);
    }

    client = std::make_shared<WsClient>(url, !insecure);
#else
    client = std::make_shared<WsClient>(url);
#endif

    std::promise<void> serverReadyPromise;
    std::shared_mutex pauseLock;
    std::shared_ptr<std::promise<void>> pausePromise;

    client->on_open = [&](const std::shared_ptr<WsClient::Connection>& connection) {
        LOG(INFO) << "WS: File download " << uuid << " connection opened to " << url;
        pConnection = connection;
    };

    client->on_error = [&](auto, auto) {
        LOG(INFO) << "WS: File download " << uuid << " connection closed uncleanly to " << url;
        pConnection = nullptr;

        std::unique_lock<std::shared_mutex> lock(pauseLock);
        if (pausePromise) {
            pausePromise->set_value();
            pausePromise = nullptr;
        }
    };

    client->on_close = [&](const std::shared_ptr<WsClient::Connection>&, int, const std::string &) {
        LOG(INFO) << "WS: File download " << uuid << " connection closed to " << url;
        pConnection = nullptr;

        std::unique_lock<std::shared_mutex> lock(pauseLock);
        if (pausePromise) {
            pausePromise->set_value();
            pausePromise = nullptr;
        }
    };

    client->on_message = [&](const std::shared_ptr<WsClient::Connection>&, const std::shared_ptr<WsClient::InMessage>& inMessage) {
        auto stringData = inMessage->string();
        auto message = std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end()));

        if (message->getId() == PAUSE_FILE_CHUNK_STREAM) {
            std::unique_lock<std::shared_mutex> lock(pauseLock);
            if (!pausePromise) {
                pausePromise = std::make_shared<std::promise<void>>();
            }

        } else if (message->getId() == RESUME_FILE_CHUNK_STREAM) {
            std::unique_lock<std::shared_mutex> lock(pauseLock);
            if (pausePromise) {
                pausePromise->set_value();
                pausePromise = nullptr;
            }
        } else if (message->getId() == SERVER_READY) {
            serverReadyPromise.set_value();
        } else {
            LOG(WARNING) << "Got invalid message ID from server: " << message->getId();
        }
    };

    auto clientThread = std::thread([&]() {
        // Start client
        client->start();
    });

    serverReadyPromise.get_future().wait();

    std::function<void()> closeConnection = [&]() {
        if (pConnection) {
            pConnection->send_close(1000, "Closing connection.");
            pConnection = nullptr;
        }
        client->stop();
        clientThread.join();
    };

    std::string workingDirectory;

    if (jobId != 0) {
        // Get the job
        sJob job;
        try {
            job = sJob::getOrCreateByJobId(jobId);

            if (job.id == 0) {
                throw std::runtime_error("Job did not exist");
            }
        } catch (std::runtime_error&) {
            LOG(ERROR) << "Job does not exist with ID " << jobId;

            // Report that the job doesn't exist
            auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string("Job does not exist");
            sendMessage(result, pConnection);

            closeConnection();
            return;
        }

        if (static_cast<bool>(job.submitting)) {
            LOG(INFO) << "Job " << jobId << " is submitting, nothing to do";

            // Report that the job hasn't been submitted
            auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string("Job is not submitted");
            sendMessage(result, pConnection);

            closeConnection();
            return;
        }

        // Get the working directory
        workingDirectory = job.workingDirectory;
    } else {
        auto bundlePath = getBundlePath();
        workingDirectory = BundleManager::Singleton()->runBundle_string("working_directory", bundleHash, filePath,
                                                                        "file_download");
    }

    // Make sure that there is no leading slash on the file path
    while (!filePath.empty() and filePath[0] == '/') {
        filePath = filePath.substr(1);
    }

    // Get the absolute path to the file and check that the path exists
    try {
        filePath = boost::filesystem::canonical(boost::filesystem::path(workingDirectory) / filePath).string();
    } catch (boost::filesystem::filesystem_error &error) {
        LOG(WARNING) << "Path to file download does not exist "
                  << (boost::filesystem::path(workingDirectory) / filePath).string();

        // Report that the file doesn't exist
        auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Path to file download does not exist");
        sendMessage(result, pConnection);

        closeConnection();
        return;
    }
    // Verify that this directory really sits under the working directory
    if (!filePath.starts_with(workingDirectory)) {
        LOG(WARNING) << "Path to file download is outside the working directory " << filePath;

        // Report that the file doesn't exist
        auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Path to file download is outside the working directory");
        sendMessage(result, pConnection);

        closeConnection();
        return;
    }

    // Verify that the path is a file
    if (!boost::filesystem::is_regular_file(filePath)) {
        LOG(WARNING) << "Path to file download is not a file " << filePath;

        // Report that the file doesn't exist
        auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Path to file download is not a file");
        sendMessage(result, pConnection);

        closeConnection();
        return;
    }

    LOG(INFO) << "Trying to download file " << jobId << " " << uuid << " " << bundleHash;
    LOG(INFO) << "Path " << filePath;

    // Get the file size
    auto fileSize = boost::filesystem::file_size(filePath);

    // Send the file size to the server
    auto result = Message(FILE_DOWNLOAD_DETAILS, Message::Priority::Highest, uuid);
    result.push_ulong(fileSize);
    sendMessage(result, pConnection);

    try {
        // Open the file and stream it to the server
        std::ifstream file(filePath, std::ios::in | std::ios::binary);

        uint64_t packetCount = 0;

        // Loop until all bytes of the file have been read or the connection is closed
        auto event = std::make_shared<std::promise<void>>();
        while (fileSize != 0 && pConnection) {
            // Check if the server has asked us to pause the stream
            std::shared_ptr<std::promise<void>> ourPausePromise;
            {
                std::unique_lock<std::shared_mutex> lock(pauseLock);
                ourPausePromise = pausePromise;
            }

            if (ourPausePromise) {
                ourPausePromise->get_future().wait();
            }

            // Read the next chunk and send it to the server
            std::vector<uint8_t> data(CHUNK_SIZE);
            // NOLINTBEGIN(cppcoreguidelines-pro-type-reinterpret-cast)
            if (fileSize > CHUNK_SIZE) {
                file.read(reinterpret_cast<char *>(data.data()), CHUNK_SIZE);
            } else {
                data.resize(fileSize);
                file.read(reinterpret_cast<char *>(data.data()), static_cast<std::streamsize>(fileSize));
            }
            // NOLINTEND(cppcoreguidelines-pro-type-reinterpret-cast)

            // Since we don't want to flood the packet scheduler (So that we can give the server a chance to
            // pause file transfers), we create an event that we wait for on every nth packet, and don't
            // transfer any more packets until the marked packet has been sent. There will always be some
            // amount of buffer overrun on the server, but at localhost speeds it's about 8Mb which is tolerable
            event = std::make_shared<std::promise<void>>();

            // Send the packet to the scheduler
            result = Message(FILE_CHUNK, Message::Priority::Lowest, uuid);
            result.push_bytes(data);
            sendMessage(result, pConnection, [event] { event->set_value(); });

            // If this is the nth packet, wait for it to be sent before sending additional packets
            if (packetCount % CHUNK_WAIT_COUNT == 0) {
                event->get_future().wait();
            }

            // Update counters
            fileSize -= data.size();
            packetCount++;
        }

        try {
            event->get_future().wait();
        } catch (std::exception&) {}

        LOG(INFO) << "Finished file transfer for " << filePath;
    } catch (std::exception &error) {
        LOG(ERROR) << "Error in file transfer";
        LOG(ERROR) << error.what();

        // Report that there was a file exception
        result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string("Exception reading file");
        try {
            sendMessage(result, pConnection);
        } catch (std::exception&) {}
    }

    closeConnection();
}

void handleFileDownload(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the file download
    auto thread = std::thread{[msg] { handleFileDownloadImpl(msg); }};
    thread.detach();
}