//
// Created by lewis on 9/8/22.
//

#include "MessageHandler.h"
#include "../Files/FileHandling.h"
#include "../Jobs/JobHandling.h"
#include "../Websocket/WebsocketInterface.h"
#include <iostream>

void handleMessage(const std::shared_ptr<Message>& message) {
    switch (message->getId()) {
        case SERVER_READY:
            // SERVER_READY is sent by the server once the server is ready to receive messages. Until we receive this
            // message, our packet scheduler should not be running
            WebsocketInterface::Singleton()->serverReady();
            break;
        case FILE_LIST:
            // List all files in a directory
            handleFileList(message);
            break;
        case SUBMIT_JOB:
            // Submit a job
            handleJobSubmit(message);
            break;
        case DB_RESPONSE:
            WebsocketInterface::Singleton()->setDbRequestResponse(message);
            break;
        case CANCEL_JOB:
            // Cancel a job
            handleJobCancel(message);
            break;
        case DELETE_JOB:
            // Delete a job
            handleJobDelete(message);
            break;
        case FILE_DOWNLOAD:
            // Download a file
            handleFileDownload(message);
            break;
        default:
            LOG(WARNING) << "Message Handler: Got unknown message ID from the server " << message->getId();
    }
}