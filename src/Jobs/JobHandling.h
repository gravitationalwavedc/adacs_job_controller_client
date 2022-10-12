//
// Created by lewis on 10/2/22.
//

#ifndef ADACS_JOB_CLIENT_JOBHANDLING_H
#define ADACS_JOB_CLIENT_JOBHANDLING_H

#include "../DB/sJob.h"
#include "../Lib/Messaging/Message.h"
#include "../Lib/jobclient_schema.h"

void handleJobSubmit(const std::shared_ptr<Message> &msg);

auto checkJobStatus(const sJob& job, bool forceNotification = false) -> std::thread;
void checkAllJobsStatus();

auto archiveJob(const sJob& job) -> bool;

#endif //ADACS_JOB_CLIENT_JOBHANDLING_H
