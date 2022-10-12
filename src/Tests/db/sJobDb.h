//
// Created by lewis on 10/5/22.
//

#ifndef ADACS_JOB_CLIENT_SJOBDB_H
#define ADACS_JOB_CLIENT_SJOBDB_H

#include "../../Lib/jobclient_schema.h"
#include "SqliteConnector.h"
#include <cstdint>
#include <string>
#include <thread>


struct sJobDb {
    static auto fromRecord(auto record) -> sJobDb {
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

    static auto getOrCreateByJobId(auto jobId) -> sJobDb {
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

    static auto getRunningJobs() -> std::vector<sJobDb> {
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
                                                _jobTable.running == 1
                                                and _jobTable.queued == 0
                                                and _jobTable.jobId != 0
                                                and _jobTable.submitting == 0
                                        )
                        );

                std::vector<sJobDb> jobs;
                for (const auto& job : jobResults) {
                    jobs.push_back(fromRecord(&job));
                }

                return jobs;
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

    void _delete() const {
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
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        throw std::runtime_error("Unable to delete record, even after retrying");
    }

    void save() { // NOLINT(readability-function-cognitive-complexity)
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJob _jobTable;

        // Retry for up to 10 seconds
        for (int count = 0; count < 100; count++) {
            try {
                if (id != 0) {
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
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        throw std::runtime_error("Unable to save record, even after retrying");
    }

    static auto getById(uint64_t id) -> sJobDb {
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

        throw std::runtime_error("Unable to refresh record, even after retrying");
    }

    void refreshFromDb() {
        if (id == 0) {
            throw std::runtime_error("Can't refresh job that isn't saved");
        }

        *this = getById(id);
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


#endif //ADACS_JOB_CLIENT_SJOBDB_H
