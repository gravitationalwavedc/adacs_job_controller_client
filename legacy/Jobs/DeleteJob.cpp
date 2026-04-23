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

void handleJobDeleteImpl(const std::shared_ptr<Message> &msg) {
    // Get the job to cancel
    auto jobId = msg->pop_uint();

    auto job = sJob::getOrCreateByJobId(jobId);
    if (job.id == 0 or job.running or job.submitting or job.deleted) {
        // Job not found or is in an invalid state, report error
        LOG(ERROR) << "Job does not exist (" << jobId << "), is currently running, or has already been deleted.";

        if (job.id == 0 or job.deleted) {
            // Notify the server that the job has been cancelled
            auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
            result.push_uint(jobId);
            result.push_string("_job_completion_");
            result.push_uint(JobStatus::DELETED);
            result.push_string("Job has been deleted");
            result.send();
        }

        return;
    }

    // If the job is currently deleting, do nothing
    if (job.deleting) {
        return;
    }

    // Mark the job as deleting
    job.deleting = true;
    job.save();

    try {
        // The job is not running and hasn't been deleted

        // Create a dict to store the data for this job
        auto details = getDefaultJobDetails();
        details["job_id"] = job.jobId;
        details["scheduler_id"] = job.schedulerId;

        // Get the status of the job
        auto bDeleted = BundleManager::Singleton()->runBundle_bool("delete", job.bundleHash, details, "");
        if (!bDeleted) {
            // If there was an issue with the bundle deleting the job, mark the job as not deleting
            // so that it can try again later
            job.deleting = false;
            job.save();

            LOG(WARNING) << "Job " << jobId << " could not be deleted by the bundle.";
            return;
        }

        // The job has been deleted so we can update the job and the server
        job.deleting = false;
        job.deleted = true;
        job.save();

        // Notify the server that the job has been deleted
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
        result.push_uint(job.jobId);
        result.push_string("_job_completion_");
        result.push_uint(JobStatus::DELETED);
        result.push_string("Job has been deleted");
        result.send();
    } catch (std::exception& except) {
        LOG(ERROR) << "Error cancelling job: " << except.what();
        dumpExceptions(except);
    }
}

void handleJobDelete(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the job cancellation
    auto thread = std::thread{[msg] { handleJobDeleteImpl(msg); }};
    thread.detach();
}