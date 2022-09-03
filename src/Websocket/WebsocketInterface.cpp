//
// Created by lewis on 9/4/22.
//

#include "WebsocketInterface.h"
#include "../utils/GeneralUtils.h"

WebsocketInterface::WebsocketInterface(std::string token) {
    auto config = readClientConfig();
    auto url = std::string(config["websocketEndpoint"]) + "?token=" + token;
    client = std::make_shared<WsClient>(url);
}
