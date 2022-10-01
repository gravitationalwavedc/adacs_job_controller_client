//
// Created by lewis on 10/2/22.
//

#include <thread>
#include "JobHandling.h"

void checkJobStatusImpl(sJob job, bool forceNotification) {

}

void checkJobStatus(sJob job, bool forceNotification) {
    // This function simply spawns a new thread to deal with checking the job status
    auto thread = std::thread{[job, forceNotification] { checkJobStatusImpl(job, forceNotification); }};
    thread.detach();
}