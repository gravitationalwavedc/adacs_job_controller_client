//
// Created by lewis on 10/2/22.
//

#ifndef ADACS_JOB_CLIENT_JOBHANDLING_H
#define ADACS_JOB_CLIENT_JOBHANDLING_H

#include "../lib/Messaging/Message.h"
#include "../lib/jobclient_schema.h"
#include "../DB/sJob.h"

void handleJobSubmit(const std::shared_ptr<Message> &msg);

auto checkJobStatus(const sJob& job, bool forceNotification = false) -> std::thread;
void checkAllJobsStatus();

auto archiveJob(const sJob& job) -> bool;

#endif //ADACS_JOB_CLIENT_JOBHANDLING_H
