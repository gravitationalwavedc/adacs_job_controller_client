//
// Created by lewis on 10/2/22.
//

#include "JobHandling.h"
#include "../Bundle/BundleManager.h"
#include "../lib/JobStatus.h"
#include <shared_mutex>

static std::shared_mutex mutex_;

void handleJobSubmitImpl(const std::shared_ptr<Message> &msg) {
    auto jobId = msg->pop_uint();
    auto bundleHash = msg->pop_string();
    auto params = msg->pop_string();

    // Create a dict to store the data for this job
    auto details = getDefaultJobDetails();
    sJob job;

    // The following fragment of code is a critical section, so we exclude access to any more than one thread. Without
    // this, it's possible that more than one job is entered in the database with the same job id
    {
        std::unique_lock<std::shared_mutex> lock(mutex_);

        // Check if this job has already been submitted
        job = sJob::getOrCreateByJobId(jobId);

        // If the job is still waiting to be submitted - there is nothing more to do
        if (job.submitting) {
            job.submittingCount++;
            if (job.submittingCount >= 10) {
                std::cout << "Job with ID " << jobId
                          << " took too long to submit - assuming it's failed and trying again..." << std::endl;
                job.submittingCount = 0;
                job.submitting = false;
                job.jobId = 0;
                job.save();
            } else {
                std::cout << "Job with ID " << jobId << " is being submitted, nothing to do" << std::endl;
                job.save();
                return;
            }
        }

        if (job.jobId != 0) {
            std::cout << "Job with ID " << jobId << " has already been submitted, checking status..." << std::endl;
            // If the job has already been submitted, check the state of the job and notify the server of its current state
            checkJobStatus(job, true);
            return;
        }

        std::cout << "Attempting to submit new job with UI ID: " << jobId << std::endl;

        // Submit the job and record that we have submitted the job
        std::cout << "Submitting new job with ui id " << jobId << std::endl;

        // Update the jobId in the details
        details['job_id'] = jobId;

        // Update the job object and save it
        job.jobId = jobId;
        job.bundleHash = bundleHash;
        job.submitting = true;
        job.workingDirectory = "";
        job.save();
    }

    // The job is guaranteed to be in the database now, so we can finish the critical section

    // Get the working directory
    job.workingDirectory = BundleManager::Singleton()->runBundle_string("working_directory", bundleHash, details, "");

    // Update the job object and save it
    job.save();

    try {
        // Run the bundle.py submit
        job.schedulerId = BundleManager::Singleton()->runBundle_uint64("submit", bundleHash, details, params);
    } catch (std::exception &except) {
        job.schedulerId = 0;
    }

    // Check if there was an issue with the job
    if (job.schedulerId == 0) {
        std::cout << "Job with UI ID " << job.jobId << " could not be submitted" << std::endl;

        job._delete();

        // Notify the server that the job is failed
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
        result.push_uint(job.jobId);
        result.push_string("system");
        result.push_uint(JobStatus::ERROR);
        result.push_string("Unable to submit job. Please check the logs as to why.");
        result.send();

        // Notify the server that the job is failed and completed
        result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
        result.push_uint(job.jobId);
        result.push_string("_job_completion_");
        result.push_uint(JobStatus::ERROR);
        result.push_string("Unable to submit job. Please check the logs as to why.");
        result.send();
    } else {
        // Update and save the job
        job.submitting = false;
        job.save();

        std::cout << "Successfully submitted job with UI ID " << job.jobId << ", got scheduler id " << job.schedulerId
                  << std::endl;

        // Notify the server that the job is submitted
        auto result = Message(UPDATE_JOB, Message::Priority::Medium, std::to_string(jobId));
        result.push_uint(job.jobId);
        result.push_string("system");
        result.push_uint(JobStatus::SUBMITTED);
        result.push_string("Job submitted successfully");
        result.send();
    }
}

void handleJobSubmit(const std::shared_ptr<Message> &msg) {
    // This function simply spawns a new thread to deal with the file listing
    auto thread = std::thread{[msg] { handleJobSubmitImpl(msg); }};
    thread.detach();
}