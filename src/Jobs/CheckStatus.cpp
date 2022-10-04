//
// Created by lewis on 10/2/22.
//

#include "JobHandling.h"
#include "../Bundle/BundleManager.h"
#include "../lib/JobStatus.h"
#include "glog/logging.h"
#include <thread>

struct sStatus {
    uint64_t id;
    uint64_t jobId;
    std::string what;
    uint32_t state;

    static auto fromDb(auto &record) -> sStatus {
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
                std::vector<sStatus> vStatus;
                for (const auto &record: statusResults) {
                    vStatus.push_back(sStatus::fromDb(record));
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
                std::vector<sStatus> vStatus;
                for (const auto &record: statusResults) {
                    vStatus.push_back(sStatus::fromDb(record));
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
                                .where(_jobStatusTable.id == sqlpp::value_list(ids))
                );
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

void checkJobStatusImpl(sJob job, bool forceNotification) {
    /*
    Checks a job to see what its current status is on the cluster. If the job state has changed since we last
    checked, then update the server. If force_notification is True, it will update the server even if the job
    status hasn't changed

    :param job: The job object to check
    :param force_notification: If we should notify the server of the job status even if it hasn't changed
    :return: Nothing
    */

    // Create a dict to store the data for this job
    auto details = getDefaultJobDetails();
    details["job_id"] = job.jobId;
    details["scheduler_id"] = job.schedulerId;

    // Get the status of the job
    auto _status = BundleManager::Singleton()->runBundle_json("status", job.bundleHash, details, "");

    // Check if the status has changed or not
    for (const auto& stat: _status["status"]) {
        auto info = stat["info"];
        auto status = stat["status"];
        auto what = stat["what"];

        auto vStatus = sStatus::getJobStatusByJobIdAndWhat(job.id, what);

        // Prevent duplicates
        if (vStatus.size() > 1) {
            std::vector<uint64_t> ids;
            transform(vStatus.begin(), vStatus.end(), ids.begin(), [](const sStatus &status) { return status.id; });

            sStatus::deleteByIdList(ids);

            // Empty the results since they're now deleted
            vStatus = {};
        }

        if (forceNotification || !vStatus.empty() || status != vStatus[0].state) {
            // Check for a valid status - sometimes schedulers return an empty string
            if (status.is_null()) {
                return;
            }

            sStatus stateItem{
                    .jobId = job.id
            };

            if (!vStatus.empty()) {
                stateItem = vStatus.front();
            }

            // Update the database
            stateItem.what = what;
            stateItem.state = status;

            stateItem.save();

            // Send the status to the server
            auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(job.jobId));
            result.push_uint(job.jobId);
            result.push_string(what);
            result.push_uint(status);
            result.push_string(info);
            result.send();
        }
    }

    auto jobError = 0U;
    auto vStatus = sStatus::getJobStatusByJobId(job.jobId);
    for (auto& state : vStatus) {
        // Check if any of the jobs are in error state
        if (state.state > JobStatus::RUNNING || state.state != JobStatus::COMPLETED) {
            jobError = state.state;
        }
    }

    auto jobComplete = true;
    for (auto& state : vStatus) {
        // Check if all jobs are complete
        if (state.state != JobStatus::COMPLETED) {
            jobComplete = false;
        }
    }

    // Check if there was an error, or if all jobs have completed
    if (jobError != 0 or (static_cast<bool>(_status['complete']) and jobComplete)) {
        job.running = false;
        job.save();

        // Tar up the job
        archiveJob(job);

        // Notify the server that the job has completed
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(job.jobId));
        result.push_uint(job.jobId);
        result.push_string("_job_completion_");
        result.push_uint(jobError != 0 ? jobError : JobStatus::COMPLETED);
        result.push_string("Job has completed");
        result.send();
    }
}

//async def check_all_jobs(con):
//    try:
//        jobs = await sync_to_async(Job.objects.filter)(running=True, queued=False, job_id__isnull=False,
//                                                       submitting=False)
//        logging.info("Jobs {}".format(str(await sync_to_async(jobs.__repr__)())))
//
//        futures = []
//        async for job in sync_to_async_iterable(jobs):
//            futures.append(asyncio.ensure_future(check_job_status(con, job)))
//
//        if len(futures):
//            await asyncio.wait(futures)
//
//    except Exception as Exp:
//        # An exception occurred, log the exception to the log
//        logging.error("Error in check job status")
//        logging.error(type(Exp))
//        logging.error(Exp.args)
//        logging.error(Exp)
//
//        # Also log the stack trace
//        exc_type, exc_value, exc_traceback = sys.exc_info()
//        lines = traceback.format_exception(exc_type, exc_value, exc_traceback)
//        logging.error(''.join('!! ' + line for line in lines))
//}

void checkJobStatus(const sJob& job, bool forceNotification) {
    // This function simply spawns a new thread to deal with checking the job status
    auto thread = std::thread{[job, forceNotification] { checkJobStatusImpl(job, forceNotification); }};
    thread.detach();
}