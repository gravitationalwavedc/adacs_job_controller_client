//
// Created by lewis on 10/5/22.
//

#ifndef ADACS_JOB_CLIENT_SSTATUSDB_H
#define ADACS_JOB_CLIENT_SSTATUSDB_H

#include "../../Lib/jobclient_schema.h"
#include "SqliteConnector.h"
#include <cstdint>
#include <string>
#include <thread>


struct sStatusDb {
    uint64_t id;
    uint64_t jobId;
    std::string what;
    uint32_t state;

    static auto fromDb(auto &record) -> sStatusDb {
        return {
                .id = static_cast<uint64_t>(record.id),
                .jobId = static_cast<uint64_t>(record.jobId),
                .what = record.what,
                .state = static_cast<uint32_t>(record.state)
        };
    }

    static auto getJobStatusByJobIdAndWhat(uint64_t jobId, const std::string& what) {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJobstatus _jobStatusTable;

        for (int count = 0; count < 100; count++) {
            try {
                auto statusResults = _database->operator()(
                        select(all_of(_jobStatusTable))
                                .from(_jobStatusTable)
                                .where(
                                        _jobStatusTable.jobId == jobId
                                        and _jobStatusTable.what == what
                                )
                );

                // Parse the objects
                std::vector<sStatusDb> vStatus;
                for (const auto &record: statusResults) {
                    vStatus.push_back(sStatusDb::fromDb(record));
                }

                return vStatus;
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

    static auto getJobStatusByJobId(uint64_t jobId) {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJobstatus _jobStatusTable;

        for (int count = 0; count < 100; count++) {
            try {
                auto statusResults = _database->operator()(
                        select(all_of(_jobStatusTable))
                                .from(_jobStatusTable)
                                .where(
                                        _jobStatusTable.jobId == jobId
                                )
                );

                // Parse the objects
                std::vector<sStatusDb> vStatus;
                for (const auto &record: statusResults) {
                    vStatus.push_back(sStatusDb::fromDb(record));
                }

                return vStatus;
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

    static void deleteByIdList(const std::vector<uint64_t>& ids) {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJobstatus _jobStatusTable;

        for (int count = 0; count < 100; count++) {
            try {
                _database->operator()(
                        remove_from(_jobStatusTable)
                                .where(_jobStatusTable.id.in(sqlpp::value_list(ids)))
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

        throw std::runtime_error("Unable to save record, even after retrying");
    }

    void save() {
        SqliteConnector _database = SqliteConnector();
        schema::JobclientJobstatus _jobStatusTable;

        // Retry for up to 10 seconds
        for (int count = 0; count < 100; count++) {
            try {
                if (id != 0) {
                    // Update the record
                    _database->operator()(
                            update(_jobStatusTable)
                                    .set(
                                            _jobStatusTable.jobId = jobId,
                                            _jobStatusTable.what = what,
                                            _jobStatusTable.state = state
                                    )
                                    .where(
                                            _jobStatusTable.id == static_cast<uint64_t>(id)
                                    )
                    );
                } else {
                    // Create the record
                    id = _database->operator()(
                            insert_into(_jobStatusTable)
                                    .set(
                                            _jobStatusTable.jobId = jobId,
                                            _jobStatusTable.what = what,
                                            _jobStatusTable.state = state
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
};

#endif //ADACS_JOB_CLIENT_SSTATUSDB_H
