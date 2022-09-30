//
// Created by lewis on 9/28/22.
//

#include "FileHandling.h"
#include "../DB/SqliteConnector.h"
#include "../lib/jobclient_schema.h"
#include <cstdint>
#include <boost/filesystem.hpp>
#include <fstream>
#include "../Bundle/BundleManager.h"
#include <future>

std::map<std::string, std::promise<void>> pausedFileTransfers;

void handleFileDownloadImpl(const std::shared_ptr<Message> &msg) {
    // Get the job details
    auto jobId = msg->pop_uint();
    auto uuid = msg->pop_string();
    auto bundleHash = msg->pop_string();
    auto filePath = msg->pop_string();

    // Create a database connection
    auto database = SqliteConnector();

    schema::JobclientJob jobTable;

    std::string workingDirectory;

    if (jobId != 0) {
        // Get the job
        auto jobResults =
                database->operator()(
                        select(all_of(jobTable))
                                .from(jobTable)
                                .where(
                                        jobTable.id == static_cast<uint64_t>(jobId)
                                )
                );

        // Check that a job was actually found
        if (jobResults.empty()) {
            std::cout << "Job does not exist with ID " << jobId << std::endl;

            // Report that the job doesn't exist
            auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string(uuid);
            result.push_string("Job does not exist");
            result.send();
            return;
        }

        const auto *job = &jobResults.front();

        if (static_cast<bool>(job->submitting)) {
            std::cout << "Job " << jobId << " is submitting, nothing to do" << std::endl;

            // Report that the job hasn't been submitted
            auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
            result.push_string(uuid);
            result.push_string("Job is not submitted");
            result.send();
            return;
        }

        // Get the working directory
        workingDirectory = job->workingDirectory;
    } else {
        auto bundlePath = getBundlePath();
        workingDirectory = BundleManager::Singleton()->runBundle("working_directory", bundleHash, filePath,
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
        std::cout << "Path to file download does not exist "
                  << (boost::filesystem::path(workingDirectory) / filePath).string() << std::endl;

        // Report that the file doesn't exist
        auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to file download does not exist");
        result.send();
        return;
    }
    // Verify that this directory really sits under the working directory
    if (!filePath.starts_with(workingDirectory)) {
        std::cout << "Path to file download is outside the working directory " << filePath << std::endl;

        // Report that the file doesn't exist
        auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to file download is outside the working directory");
        result.send();
        return;
    }

    // Verify that the path is a file
    if (!boost::filesystem::is_regular_file(filePath)) {
        std::cout << "Path to file download is not a file " << filePath << std::endl;

        // Report that the file doesn't exist
        auto result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to file download is not a file");
        result.send();
        return;
    }

    std::cout << "Trying to download file " << jobId << " " << uuid << " " << bundleHash << std::endl;
    std::cout << "Path " << filePath << std::endl;

    // Get the file size
    auto fileSize = boost::filesystem::file_size(filePath);

    // Send the file size to the server
    auto result = Message(FILE_DOWNLOAD_DETAILS, Message::Priority::Highest, uuid);
    result.push_string(uuid);
    result.push_ulong(fileSize);
    result.send();

    try {
        // Open the file and stream it to the server
        std::ifstream file(filePath, std::ios::in | std::ios::binary);

        uint64_t packetCount = 0;

        // Loop until all bytes of the file have been read
        while (fileSize != 0) {
            // Check if the server has asked us to pause the stream
            auto pausePromise = pausedFileTransfers.find(uuid);
            if (pausePromise != pausedFileTransfers.end()) {
                (*pausePromise).second.get_future().wait();
            }

            // Read the next chunk and send it to the server
            std::vector<uint8_t> data(CHUNK_SIZE);
            if (fileSize > CHUNK_SIZE) {
                file.read(reinterpret_cast<char*>(data.data()), CHUNK_SIZE);
            } else {
                data.resize(fileSize);
                file.read(reinterpret_cast<char*>(data.data()), fileSize);
            }

            // Since we don't want to flood the packet scheduler (So that we can give the server a chance to
            // pause file transfers), we create an event that we wait for on every nth packet, and don't
            // transfer any more packets until the marked packet has been sent. There will always be some
            // amount of buffer overrun on the server, but at localhost speeds it's about 8Mb which is tolerable
            auto event = std::make_shared<std::promise<void>>();

            // Send the packet to the scheduler
            result = Message(FILE_CHUNK, Message::Priority::Lowest, uuid, [event] { event->set_value(); });
            result.push_string(uuid);
            result.push_bytes(data);
            result.send();

            // If this is the nth packet, wait for it to be sent before sending additional packets
            if (packetCount % CHUNK_WAIT_COUNT == 0) {
                event->get_future().wait();
            }

            // Update counters
            fileSize -= data.size();
            packetCount++;
        }

        std::cout << "Finished file transfer for " << filePath << std::endl;
    } catch (std::exception &error) {
        std::cerr << "Error in file transfer" << std::endl;
        std::cerr << error.what() << std::endl;

        // Report that there was a file exception
        result = Message(FILE_DOWNLOAD_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Exception reading file");
        result.send();
    }
}

void handleFileDownload(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the file download
    auto thread = std::thread{[msg] { handleFileDownloadImpl(msg); }};
    thread.detach();
}