//
// Created by lewis on 10/2/22.
//

#include "JobHandling.h"
#include "../Bundle/BundleManager.h"
#include "../DB/sStatus.h"
#include "../Lib/JobStatus.h"
#include "glog/logging.h"
#include <thread>

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

    LOG(INFO) << "A. Status";

    // Get the status of the job
    auto _status = BundleManager::Singleton()->runBundle_json("status", job.bundleHash, details, "");

    LOG(INFO) << "A. Status Done";

    // Check if the status has changed or not
    for (const auto& stat: _status["status"]) {
        auto info = stat["info"];
        auto jsonStatus = stat["status"];
        auto what = stat["what"];

        // Check for a valid status - sometimes schedulers return an empty string
        if (jsonStatus.is_null()) {
            LOG(INFO) << "A. jsonStatus was null";
            return;
        }

        auto status = static_cast<uint32_t>(jsonStatus);

        LOG(INFO) << "A. getJobStatusByJobIdAndWhat";
        auto vStatus = sStatus::getJobStatusByJobIdAndWhat(job.id, what);

        LOG(INFO) << "A. getJobStatusByJobIdAndWhat Done";
        // Prevent duplicates
        if (vStatus.size() > 1) {
            std::vector<uint64_t> ids;
            transform(vStatus.begin(), vStatus.end(), std::back_inserter(ids), [](const sStatus &status) { return status.id; });

            LOG(INFO) << "A. deleteByIdList";
            sStatus::deleteByIdList(ids);

            // Empty the results since they're now deleted
            vStatus = {};
        }

        if (forceNotification || vStatus.empty() || status != vStatus[0].state) {
            LOG(INFO) << "A. Doing update";
            sStatus stateItem{
                    .jobId = job.id
            };

            if (!vStatus.empty()) {
                stateItem = vStatus.front();
            }

            // Update the database
            stateItem.what = what;
            stateItem.state = status;

            LOG(INFO) << "A. Save stateItem";
            stateItem.save();

            LOG(INFO) << "A. Send update message on ws";
            // Send the status to the server
            auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(job.jobId));
            result.push_uint(job.jobId);
            result.push_string(what);
            result.push_uint(status);
            result.push_string(info);
            result.send();
            LOG(INFO) << "A. update message on ws done";
        }
    }

    LOG(INFO) << "A. getJobStatusByJobId";
    auto jobError = 0U;
    auto vStatus = sStatus::getJobStatusByJobId(job.id);
    LOG(INFO) << "A. getJobStatusByJobId Done";
    for (auto& state : vStatus) {
        // Check if any of the jobs are in error state
        if (state.state > JobStatus::RUNNING and state.state != JobStatus::COMPLETED) {
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
    if (jobError != 0 or (static_cast<bool>(_status["complete"]) and jobComplete)) {
        LOG(INFO) << "A. Job Complete save";
        job.running = false;
        job.save();

        LOG(INFO) << "A. Archive Job";
        // Tar up the job
        archiveJob(job);

        LOG(INFO) << "A. Send job completion message on ws";
        // Notify the server that the job has completed
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(job.jobId));
        result.push_uint(job.jobId);
        result.push_string("_job_completion_");
        result.push_uint(jobError != 0 ? jobError : JobStatus::COMPLETED);
        result.push_string("Job has completed");
        result.send();
        LOG(INFO) << "A. Send job completion messag on ws done";
    }
}

auto checkJobStatus(const sJob& job, bool forceNotification) -> std::thread {
    // This function simply spawns a new thread to deal with checking the job status
    return std::thread{[job, forceNotification] {
        try {
            checkJobStatusImpl(job, forceNotification);
        } catch (const std::exception& except) {
            LOG(ERROR) << "Error getting job status: " << except.what();
            dumpExceptions(except);
        }
    }};
}

void checkAllJobsStatus() {
    std::vector<std::thread> checkThreads;

    // Get all running jobs
    LOG(INFO) << "Checking status of running jobs...";
    auto jobs = sJob::getRunningJobs();
    checkThreads.reserve(jobs.size());
    LOG(INFO) << "There are " << jobs.size() << " running jobs.";

    // Start checking the status of all jobs
    for (const auto& job : jobs) {
        checkThreads.push_back(checkJobStatus(job, false));
    }

    LOG(INFO) << "Joining...";
    // Wait for all status checks to finish
    for (auto& thread : checkThreads) {
        if (thread.joinable()) {
            thread.join();
        }
    }
    LOG(INFO) << "Joined.";
}
