//
// Created by lewis on 9/28/22.
//

#include "FileHandling.h"
#include "../DB/SqliteConnector.h"
#include "../lib/jobclient_schema.h"
#include <cstdint>
#include <boost/filesystem.hpp>
#include "../Bundle/BundleManager.h"
#include "glog/logging.h"

void handleFileListImpl(const std::shared_ptr<Message> &msg) {
    // Get the job details
    auto jobId = msg->pop_uint();
    auto uuid = msg->pop_string();
    auto bundleHash = msg->pop_string();
    auto dirPath = msg->pop_string();
    auto isRecursive = msg->pop_bool();

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
            LOG(ERROR) << "Job does not exist with ID " << jobId;

            // Report that the job doesn't exist
            auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
            result.push_string(uuid);
            result.push_string("Job does not exist");
            result.send();
            return;
        }

        const auto *job = &jobResults.front();

        if (static_cast<bool>(job->submitting)) {
            LOG(INFO) << "Job " << jobId << " is submitting, nothing to do";

            // Report that the job hasn't been submitted
            auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
            result.push_string(uuid);
            result.push_string("Job is not submitted");
            result.send();
            return;
        }

        // Get the working directory
        workingDirectory = job->workingDirectory;
    } else {
        auto bundlePath = getBundlePath();
        workingDirectory = BundleManager::Singleton()->runBundle_string("working_directory", bundleHash, dirPath,
                                                                        "file_list");
    }

    // Get the absolute path to the directory and check that the path exists
    try {
        dirPath = boost::filesystem::canonical(boost::filesystem::path(workingDirectory) / dirPath).string();
    } catch (boost::filesystem::filesystem_error &error) {
        LOG(WARNING) << "Path to list files does not exist "
                  << (boost::filesystem::path(workingDirectory) / dirPath).string();

        // Report that the file doesn't exist
        auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to list files does not exist");
        result.send();
        return;
    }
    // Verify that this directory really sits under the working directory
    if (!dirPath.starts_with(workingDirectory)) {
        LOG(WARNING) << "Path to list files is outside the working directory " << dirPath;

        // Report that the file doesn't exist
        auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to list files is outside the working directory");
        result.send();
        return;
    }

    // Verify that the path is a directory
    if (!boost::filesystem::is_directory(dirPath)) {
        LOG(WARNING) << "Path to list files is not a directory " << dirPath;

        // Report that the file doesn't exist
        auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to list files is not a directory");
        result.send();
        return;
    }

    LOG(INFO) << "Trying to get file list " << jobId << " " << uuid << " " << bundleHash;
    LOG(INFO) << "Path " << dirPath;

    // Define a struct and vector for tracking the file information
    struct sFile {
        std::string path;
        bool isDir;
        uint64_t size;
    };

    std::vector<sFile> fileList;

    // Process the file list as required
    if (isRecursive) {
        boost::filesystem::recursive_directory_iterator dirIter(dirPath);
        boost::filesystem::recursive_directory_iterator dirIterEnd;
        for (; dirIter != dirIterEnd; dirIter++) {
            // Ignore if this file is a symlink
            if (boost::filesystem::is_symlink((*dirIter))) {
                continue;
            }

            bool isDir = boost::filesystem::is_directory((*dirIter));
            fileList.push_back(
                    {
                            (*dirIter).path().string().substr(workingDirectory.length()),
                            isDir,
                            isDir ? 0 : boost::filesystem::file_size(*dirIter)
                    }
            );
        }
    } else {
        boost::filesystem::directory_iterator dirIter(dirPath);
        boost::filesystem::directory_iterator dirIterEnd;
        for (; dirIter != dirIterEnd; dirIter++) {
            // Ignore if this file is a symlink
            if (boost::filesystem::is_symlink((*dirIter))) {
                continue;
            }

            bool isDir = boost::filesystem::is_directory((*dirIter));
            fileList.push_back(
                    {
                            (*dirIter).path().string().substr(workingDirectory.length()),
                            isDir,
                            isDir ? 0 : boost::filesystem::file_size(*dirIter)
                    }
            );
        }
    }

    // Create the response message and send it
    auto result = Message(FILE_LIST, Message::Priority::Highest, uuid);
    result.push_string(uuid);
    result.push_uint(fileList.size());
    for (const auto &file: fileList) {
        result.push_string(file.path);
        result.push_bool(file.isDir);
        result.push_ulong(file.size);
    }
    result.send();

    LOG(INFO) << "File list for path " << dirPath << " completed.";
}

void handleFileList(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the file listing
    auto thread = std::thread{[msg] { handleFileListImpl(msg); }};
    thread.detach();
}