//
// Created by lewis on 9/28/22.
//

#include "FileHandling.h"
#include "../DB/SqliteConnector.h"
#include "../lib/jobclient_schema.h"
#include <cstdint>
#include <boost/filesystem.hpp>

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
            std::cout << "Job does not exist with ID " << jobId << std::endl;

            // Report that the job doesn't exist
            auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
            result.push_string(uuid);
            result.push_string("Job does not exist");
            result.send();
            return;
        }

        const auto *job = &jobResults.front();

        if (static_cast<bool>(job->submitting)) {
            std::cout << "Job " << jobId << " is submitting, nothing to do" << std::endl;

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
        std::cerr << "Bundle interface not complete" << std::endl;
        abort();
//        auto bundle_path = get_bundle_path()
//        working_directory = await run_bundle("working_directory", bundle_path, bundle_hash, dir_path, "file_list")
    }

    // Get the absolute path to the directory and check that the path exists
    try {
        dirPath = boost::filesystem::canonical(boost::filesystem::path(workingDirectory) / dirPath).string();
        std::cout << "dirPath " << dirPath << std::endl;
    } catch (boost::filesystem::filesystem_error &error) {
        std::cout << "Path to list files does not exist "
                  << (boost::filesystem::path(workingDirectory) / dirPath).string() << std::endl;

        // Report that the file doesn't exist
        auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to list files does not exist");
        result.send();
        return;
    }
    // Verify that this directory really sits under the working directory
    if (!dirPath.starts_with(workingDirectory)) {
        std::cout << "Path to list files is outside the working directory " << dirPath << std::endl;

        // Report that the file doesn't exist
        auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to list files is outside the working directory");
        result.send();
        return;
    }

    // Verify that the path is a directory
    if (!boost::filesystem::is_directory(dirPath)) {
        std::cout << "Path to list files is not a directory " << dirPath << std::endl;

        // Report that the file doesn't exist
        auto result = Message(FILE_LIST_ERROR, Message::Priority::Highest, uuid);
        result.push_string(uuid);
        result.push_string("Path to list files is not a directory");
        result.send();
        return;
    }

    std::cout << "Trying to get file list " << jobId << " " << uuid << " " << bundleHash << std::endl;
    std::cout << "Path " << dirPath << std::endl;

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
    for (const auto& file : fileList) {
        result.push_string(file.path);
        result.push_bool(file.isDir);
        result.push_ulong(file.size);
    }
    result.send();

    std::cout << "File list for path " << dirPath << " completed." << std::endl;
}

void handleFileList(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the file listing
    auto thread = std::thread{[msg] { handleFileListImpl(msg); }};
    thread.detach();
}