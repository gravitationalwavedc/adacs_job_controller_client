//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H
#define ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H

#include "client_wss.hpp"

using WsClient = SimpleWeb::SocketClient<SimpleWeb::WSS>;

class WebsocketInterface {
public:
    explicit WebsocketInterface(std::string token);

private:
    std::shared_ptr<WsClient> client;
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H
