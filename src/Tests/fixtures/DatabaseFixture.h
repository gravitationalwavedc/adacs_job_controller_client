//
// Created by lewis on 7/25/22.
//

#ifndef GWCLOUD_JOB_SERVER_DATABASEFIXTURE_H
#define GWCLOUD_JOB_SERVER_DATABASEFIXTURE_H

#include "../../Lib/jobclient_schema.h"
#include "../db/SqliteConnector.h"
#include "../db/sJobDb.h"
#include "../db/sStatusDb.h"

struct DatabaseFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    SqliteConnector database;

    schema::JobclientJob jobTable{};
    schema::JobclientJobstatus statusTable{};
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    DatabaseFixture() {
        cleanDatabase();
    }

    // NOLINTNEXTLINE(bugprone-exception-escape)
    ~DatabaseFixture() {
        cleanDatabase();
    }

    DatabaseFixture(DatabaseFixture const&) = delete;
    auto operator =(DatabaseFixture const&) -> DatabaseFixture& = delete;
    DatabaseFixture(DatabaseFixture&&) = delete;
    auto operator=(DatabaseFixture&&) -> DatabaseFixture& = delete;

    auto maybeHandleDbMessage(auto connection, auto message) -> bool {
        switch (message->getId()) {
            case DB_JOB_GET_BY_JOB_ID: {
                auto dbRequestId = message->pop_ulong();
                auto job = sJobDb::getOrCreateByJobId(message->pop_ulong());
                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_uint(1);
                result.push_ulong(job.id);
                result.push_ulong(job.jobId);
                result.push_ulong(job.schedulerId);
                result.push_bool(job.submitting);
                result.push_uint(job.submittingCount);
                result.push_string(job.bundleHash);
                result.push_string(job.workingDirectory);
                result.push_bool(job.running);
                result.send(connection);
                return true;
            }
            case DB_JOB_GET_BY_ID: {
                auto dbRequestId = message->pop_ulong();
                auto job = sJobDb::getById(message->pop_ulong());
                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_uint(1);
                result.push_ulong(job.id);
                result.push_ulong(job.jobId);
                result.push_ulong(job.schedulerId);
                result.push_bool(job.submitting);
                result.push_uint(job.submittingCount);
                result.push_string(job.bundleHash);
                result.push_string(job.workingDirectory);
                result.push_bool(job.running);
                result.send(connection);
                return true;
            }
            case DB_JOB_GET_RUNNING_JOBS: {
                auto dbRequestId = message->pop_ulong();
                auto jobs = sJobDb::getRunningJobs();
                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_uint(jobs.size());
                for (const auto& job : jobs) {
                    result.push_ulong(job.id);
                    result.push_ulong(job.jobId);
                    result.push_ulong(job.schedulerId);
                    result.push_bool(job.submitting);
                    result.push_uint(job.submittingCount);
                    result.push_string(job.bundleHash);
                    result.push_string(job.workingDirectory);
                    result.push_bool(job.running);
                }
                result.send(connection);
                return true;
            }
            case DB_JOB_DELETE: {
                auto dbRequestId = message->pop_ulong();
                sJobDb job = {
                        .id = message->pop_ulong()
                };
                job._delete();
                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.send(connection);
                return true;
            }
            case DB_JOB_SAVE: {
                auto dbRequestId = message->pop_ulong();

                sJobDb job = {
                        .id = message->pop_ulong(),
                        .jobId = message->pop_ulong(),
                        .schedulerId = message->pop_ulong(),
                        .submitting = message->pop_bool(),
                        .submittingCount = message->pop_uint(),
                        .bundleHash = message->pop_string(),
                        .workingDirectory = message->pop_string(),
                        .running = message->pop_bool()
                };

                job.save();

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_ulong(job.id);
                result.send(connection);
                return true;
            }
            case DB_JOBSTATUS_GET_BY_JOB_ID_AND_WHAT: {
                auto dbRequestId = message->pop_ulong();

                auto jobId = message->pop_ulong();
                auto what = message->pop_string();
                auto statuses = sStatusDb::getJobStatusByJobIdAndWhat(jobId, what);

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_uint(statuses.size());
                for (const auto& status : statuses) {
                    result.push_ulong(status.id);
                    result.push_ulong(status.jobId);
                    result.push_string(status.what);
                    result.push_uint(status.state);
                }
                result.send(connection);
                return true;
            }
            case DB_JOBSTATUS_GET_BY_JOB_ID: {
                auto dbRequestId = message->pop_ulong();
                auto statuses = sStatusDb::getJobStatusByJobId(message->pop_ulong());
                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_uint(statuses.size());
                for (const auto& status : statuses) {
                    result.push_ulong(status.id);
                    result.push_ulong(status.jobId);
                    result.push_string(status.what);
                    result.push_uint(status.state);
                }
                result.send(connection);
                return true;
            }
            case DB_JOBSTATUS_DELETE_BY_ID_LIST: {
                auto dbRequestId = message->pop_ulong();

                std::vector<uint64_t> ids;
                auto count = message->pop_uint();
                for (uint32_t index = 0; index < count; index++) {
                    ids.push_back(message->pop_ulong());
                }

                sStatusDb::deleteByIdList(ids);

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.send(connection);
                return true;
            }
            case DB_JOBSTATUS_SAVE: {
                auto dbRequestId = message->pop_ulong();

                sStatusDb status = {
                        .id = message->pop_ulong(),
                        .jobId = message->pop_ulong(),
                        .what = message->pop_string(),
                        .state = message->pop_uint()
                };

                status.save();

                auto result = Message(DB_RESPONSE, Message::Priority::Medium, "database");
                result.push_ulong(dbRequestId);
                result.push_bool(true);
                result.push_ulong(status.id);
                result.send(connection);
                return true;
            }
        }

        return false;
    }

private:
    void cleanDatabase() const {
        // Sanitize all records from the database
        database->operator()(remove_from(statusTable).unconditionally());
        database->operator()(remove_from(jobTable).unconditionally());
    }
};

#endif //GWCLOUD_JOB_SERVER_DATABASEFIXTURE_H
