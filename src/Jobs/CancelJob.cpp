//
// Created by lewis on 10/20/22.
//

#include "../Bundle/BundleManager.h"
#include "../DB/sJob.h"
#include "../DB/sStatus.h"
#include "../Lib/JobStatus.h"
#include "../Lib/Messaging/Message.h"
#include "JobHandling.h"
#include "glog/logging.h"
#include <memory>

void handleJobCancelImpl(const std::shared_ptr<Message> &msg) {
    // Get the job to cancel
    auto jobId = msg->pop_uint();

    auto job = sJob::getOrCreateByJobId(jobId);
    if (job.id == 0 or job.running == false or job.submitting == true) {
        // Job not found or is in an invalid state, report error
        LOG(ERROR) << "Job does not exist (" << jobId << "), or job is in an invalid state";

        // Notify the server that the job has been cancelled
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
        result.push_uint(jobId);
        result.push_string("_job_completion_");
        result.push_uint(JobStatus::CANCELLED);
        result.push_string("Job has been cancelled");
        result.send();

        return;
    }

    try {
        // Force a status check
        checkJobStatus(job).join();
        job.refreshFromDb();

        // Check if the job is running after the status update
        if (job.running == false) {
            // The job is no longer running - there is nothing to do
            LOG(WARNING) << "Job " << jobId << " is not running so cannot be cancelled, nothing to do.";
            return;
        }

        // The job is still running, so attempt to call the bundle to cancel the job

        // Create a dict to store the data for this job
        auto details = getDefaultJobDetails();
        details["job_id"] = job.jobId;
        details["scheduler_id"] = job.schedulerId;

        // Get the status of the job
        auto bCancelled = BundleManager::Singleton()->runBundle_bool("cancel", job.bundleHash, details, "");
        if (bCancelled == false) {
            LOG(WARNING) << "Job " << jobId << " could not be cancelled by the bundle.";
            return;
        }

        // If the job was cancelled, we need to check the job status once more to update the server of any changes.
        // Once the final job status check has completed, we check if there is a job status already for cancelled,
        // and add our own if not

        job.refreshFromDb();
        checkJobStatus(job).join();

        auto dbStatus = sStatus::getJobStatusByJobId(jobId);
        if (std::any_of(dbStatus.begin(), dbStatus.end(), [](auto element) { return element.state == JobStatus::CANCELLED; } )) {
            // We've already notified the server of the CANCELLED status if a state record exists
            return;
        }

        // Mark the job as no longer running
        job.running = false;
        job.save();

        // Tar up the job
        archiveJob(job);

        // Notify the server that the job has been cancelled
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
        result.push_uint(job.jobId);
        result.push_string("_job_completion_");
        result.push_uint(JobStatus::CANCELLED);
        result.push_string("Job has been cancelled");
        result.send();
    } catch (std::exception& except) {
        LOG(ERROR) << "Error cancelling job: " << except.what();
        dumpExceptions(except);
    }
}

void handleJobCancel(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the job cancellation
    auto thread = std::thread{[msg] { handleJobCancelImpl(msg); }};
    thread.detach();
}