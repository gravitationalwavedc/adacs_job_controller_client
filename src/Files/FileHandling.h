//
// Created by lewis on 9/28/22.
//

#ifndef ADACS_JOB_CLIENT_FILEHANDLING_H
#define ADACS_JOB_CLIENT_FILEHANDLING_H

#include "../Lib/Messaging/Message.h"

void handleFileList(const std::shared_ptr<Message>& msg);
void handleFileDownload(const std::shared_ptr<Message>& msg);

#endif //ADACS_JOB_CLIENT_FILEHANDLING_H
