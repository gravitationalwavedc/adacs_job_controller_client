//
// Created by lewis on 9/8/22.
//

#include <iostream>
#include "MessageHandler.h"
#include "../Websocket/WebsocketInterface.h"
#include "../Files/FileHandling.h"


void handleMessage(const std::shared_ptr<Message>& message) {
    switch (message->getId()) {
        case SERVER_READY:
            // SERVER_READY is sent by the server once the server is ready to receive messages. Until we receive this
            // message, our packet scheduler should not be running
            WebsocketInterface::Singleton()->serverReady();
            break;
        case FILE_LIST:
            handleFileList(message);
            break;
        default:
            std::cerr << "Message Handler: Got unknown message ID from the server " << message->getId() << std::endl;
    }
}