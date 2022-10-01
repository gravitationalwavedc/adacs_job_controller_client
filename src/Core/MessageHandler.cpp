//
// Created by lewis on 9/8/22.
//

#include <iostream>
#include "MessageHandler.h"
#include "../Websocket/WebsocketInterface.h"
#include "../Files/FileHandling.h"

extern std::map<std::string, std::promise<void>> pausedFileTransfers;

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
        case FILE_DOWNLOAD:
            // Download a file
            handleFileDownload(message);
            break;
        case PAUSE_FILE_CHUNK_STREAM:
            // Pause a file download (Remote end's transmission buffer is above the "high" threshold)
            pausedFileTransfers.try_emplace(message->pop_string(), std::promise<void>());
            break;
        case RESUME_FILE_CHUNK_STREAM: {
            // Resume a file download (Remote end's transmission buffer is below the "low" threshold)
            auto prom = pausedFileTransfers.find(message->pop_string());
            if (prom != pausedFileTransfers.end()) {
                prom->second.set_value();
                pausedFileTransfers.erase(prom);
            }
            break;
        }
        default:
            std::cerr << "Message Handler: Got unknown message ID from the server " << message->getId() << std::endl;
    }
}