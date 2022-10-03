//
// Created by lewis on 10/2/22.
//

#include <thread>
#include "JobHandling.h"
#include "../Bundle/BundleManager.h"

struct sStatus {
    uint64_t id;
    std::string what;
    uint32_t state;

    static sStatus fromDb(auto& record) {
        return {
                .id = static_cast<uint64_t>(record.id),
                .what = record.what,
                .state = static_cast<uint32_t>(record.state)
        };
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

    SqliteConnector database = SqliteConnector();
    schema::JobclientJobstatus jobStatusTable;

    // Create a dict to store the data for this job
    auto details = getDefaultJobDetails();
    details["job_id"] = job.jobId;
    details["schedulerId"] = job.schedulerId;

    // Get the status of the job
    auto _status = BundleManager::Singleton()->runBundle_json("status", job.bundleHash, details, "");

    // Check if the status has changed or not
    for (auto stat : _status["status"]) {
        auto info = stat["info"];
        auto status = stat["status"];
        auto what = stat["what"];

        auto statusResults = database->operator()(
                select(all_of(jobStatusTable))
                        .from(jobStatusTable)
                        .where(
                                jobStatusTable.jobId == job.id
                                and jobStatusTable.what == std::string{what}
                        )
        );

        // Parse the objects
        std::vector <sStatus> vStatus;
        for (const auto &record: statusResults) {
            vStatus.push_back(sStatus::fromDb(record));
        }
        // Prevent duplicates
        if (vStatus.size() > 1) {
            std::vector<int> ids;
            transform(vStatus.begin(), vStatus.end(), ids.begin(), [](const sStatus &status) { return status.id; });

            database->operator()(
                    remove_from(jobStatusTable)
                            .where(jobStatusTable.id == sqlpp::value_list(ids))
            );

            // Empty the results since they're now deleted
            vStatus = {};
        }

        if (forceNotification || !vStatus.empty() || status != vStatus[0].state) {
            // Check for a valid status - sometimes schedulers return an empty string
            if (status.is_null()) {
                return;
            }

//            if (!vStatus.empty()) {
//
//            }

//            if await sync_to_async(dbstatus.exists)():
//                s = await sync_to_async(dbstatus.first)()
//            else:
//                s = JobStatusModel(job=job)
//
//            # Update the database
//            s.what = what
//            s.state = status
//            await sync_to_async(s.save)()
//
//            # Send the status to the server
//            result = Message(
//                UPDATE_JOB,
//                source=job.bundle_hash + "_" + str(job.job_id),
//                priority=PacketScheduler.Priority.Medium
//            )
//            result.push_uint(job.job_id)
//            result.push_string(what)
//            result.push_uint(status)
//            result.push_string(info)
//            await con.scheduler.queue_message(result)
        }
    }

//    job_error = False
//    async for state in sync_to_async_iterable(job.status.all()):
//        # Check if any of the jobs are in error state
//        if state.state > JobStatus.RUNNING and state.state != JobStatus.COMPLETED:
//            job_error = state.state
//
//    job_complete = True
//    async for state in sync_to_async_iterable(job.status.all()):
//        # Check if all jobs are complete
//        if state.state != JobStatus.COMPLETED:
//            job_complete = False
//
//    # Check if there was an error, or if all jobs have completed
//    if job_error or (_status['complete'] and job_complete >= 1):
//        job.running = False
//        await sync_to_async(job.save)()
//
//        # Tar up the job
//        await archive_job(job)
//
//        # Notify the server that the job has completed
//        result = Message(UPDATE_JOB, source=str(job.job_id), priority=PacketScheduler.Priority.Medium)
//        result.push_uint(job.job_id)
//        result.push_string("_job_completion_")
//        result.push_uint(job_error if job_error else JobStatus.COMPLETED)
//        result.push_string("Job has completed")
//        # Send the result
//        await con.scheduler.queue_message(result)
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

void checkJobStatus(sJob job, bool forceNotification) {
    // This function simply spawns a new thread to deal with checking the job status
    auto thread = std::thread{[job, forceNotification] { checkJobStatusImpl(job, forceNotification); }};
    thread.detach();
}