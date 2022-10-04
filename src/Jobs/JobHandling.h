//
// Created by lewis on 10/2/22.
//

#ifndef ADACS_JOB_CLIENT_JOBHANDLING_H
#define ADACS_JOB_CLIENT_JOBHANDLING_H

#include "../lib/Messaging/Message.h"
#include "../DB/SqliteConnector.h"
#include "../lib/jobclient_schema.h"

struct sJob {
    static sJob fromRecord(auto record) {
        return {
                static_cast<uint64_t>(record->id),
                static_cast<uint64_t>(record->jobId),
                static_cast<uint64_t>(record->schedulerId),
                static_cast<uint32_t>(record->submitting) == 1,
                static_cast<uint32_t>(record->submittingCount),
                record->bundleHash,
                record->workingDirectory,
                static_cast<uint32_t>(record->queued) == 1,
                record->params,
                static_cast<uint32_t>(record->running) == 1
        };
    }

    static sJob getOrCreateByJobId(auto jobId) {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJob _jobTable;

        // Retry for up to 10 seconds
        for (int count = 0; count < 100; count++) {
            try {
                auto jobResults =
                        _database->operator()(
                                select(all_of(_jobTable))
                                        .from(_jobTable)
                                        .where(
                                                _jobTable.jobId == static_cast<uint64_t>(jobId)
                                        )
                        );

                if (!jobResults.empty()) {
                    return fromRecord(&jobResults.front());
                }

                return {};
            } catch (sqlpp::exception &except) {
                if (std::string(except.what()).find("database is locked") != std::string::npos) {
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        throw std::runtime_error("Unable to get record, even after retrying");
    }

    void _delete() {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJob _jobTable;

        // Retry for up to 10 seconds
        for (int count = 0; count < 100; count++) {
            try {
                _database->operator()(
                        remove_from(_jobTable)
                                .where(
                                        _jobTable.id == static_cast<uint64_t>(id)
                                )
                );

                return;
            } catch (sqlpp::exception &except) {
                if (std::string(except.what()).find("database is locked") != std::string::npos) {
                    std::cout << "Locked" << std::endl;
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        throw std::runtime_error("Unable to delete record, even after retrying");
    }

    void save() {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJob _jobTable;

        // Retry for up to 10 seconds
        for (int count = 0; count < 100; count++) {
            try {
                if (id) {
                    // Update the record
                    _database->operator()(
                            update(_jobTable)
                                    .set(
                                            _jobTable.jobId = jobId,
                                            _jobTable.schedulerId = schedulerId,
                                            _jobTable.submitting = submitting ? 1 : 0,
                                            _jobTable.submittingCount = submittingCount,
                                            _jobTable.bundleHash = bundleHash,
                                            _jobTable.workingDirectory = workingDirectory,
                                            _jobTable.queued = queued ? 1 : 0,
                                            _jobTable.params = params,
                                            _jobTable.running = running ? 1 : 0
                                    )
                                    .where(
                                            _jobTable.id == static_cast<uint64_t>(id)
                                    )
                    );
                } else {
                    // Create the record
                    id = _database->operator()(
                            insert_into(_jobTable)
                                    .set(
                                            _jobTable.jobId = jobId,
                                            _jobTable.schedulerId = schedulerId,
                                            _jobTable.submitting = submitting ? 1 : 0,
                                            _jobTable.submittingCount = submittingCount,
                                            _jobTable.bundleHash = bundleHash,
                                            _jobTable.workingDirectory = workingDirectory,
                                            _jobTable.queued = queued ? 1 : 0,
                                            _jobTable.params = params,
                                            _jobTable.running = running ? 1 : 0
                                    )
                    );
                }
                return;
            } catch (sqlpp::exception &except) {
                if (std::string(except.what()).find("database is locked") != std::string::npos) {
                    std::cout << "Locked" << std::endl;
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        throw std::runtime_error("Unable to save record, even after retrying");
    }

    void refreshFromDb() {
        if (!id) {
            throw std::runtime_error("Can't refresh a record without an id");
        }

        SqliteConnector _database = SqliteConnector();
        schema::JobclientJob _jobTable;

        // Retry for up to 10 seconds
        for (int count = 0; count < 100; count++) {
            try {
                auto jobResults =
                        _database->operator()(
                                select(all_of(_jobTable))
                                        .from(_jobTable)
                                        .where(
                                                _jobTable.id == static_cast<uint64_t>(id)
                                        )
                        );

                if (!jobResults.empty()) {
                    *this = fromRecord(&jobResults.front());
                }
                return;
            } catch (sqlpp::exception &except) {
                if (std::string(except.what()).find("database is locked") != std::string::npos) {
                    std::cout << "Locked" << std::endl;
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        throw std::runtime_error("Unable to refresh record, even after retrying");
    }

    uint64_t id = 0;
    uint64_t jobId = 0;
    uint64_t schedulerId = 0;
    bool submitting = false;
    uint32_t submittingCount = 0;
    std::string bundleHash;
    std::string workingDirectory;
    bool queued = false;
    std::string params;
    bool running = false;
};

void handleJobSubmit(const std::shared_ptr<Message> &msg);

void checkJobStatus(const sJob& job, bool forceNotification = false);

bool archiveJob(const sJob& job);

#endif //ADACS_JOB_CLIENT_JOBHANDLING_H
