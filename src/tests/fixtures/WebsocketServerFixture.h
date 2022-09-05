//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
#define ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H

#include "../../Settings.h"
#include "../../lib/GeneralUtils.h"
#include "../../tests/utils.h"
#include "../utils.h"
#include "JsonConfigFixture.h"
#include <boost/test/unit_test.hpp>
#include <fstream>

class WebsocketServerFixture : public JsonConfigFixture {
public:
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::shared_ptr<TestWsServer> websocketServer;
    std::thread serverThread;
    std::promise<std::shared_ptr<TestWsServer::Connection>> pWebsocketServerConnection;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    WebsocketServerFixture() {
        websocketServer = std::make_shared<TestWsServer>();
        websocketServer->config.port = TEST_SERVER_PORT;

        websocketServer->endpoint["^(.*?)$"].on_open = [&]([[maybe_unused]] auto connection) {
            pWebsocketServerConnection.set_value(connection);
            onWebsocketServerOpen(connection);
        };

        websocketServer->endpoint["^(.*?)$"].on_message = [&]([[maybe_unused]] auto connection, auto in_message) {
            onWebsocketServerMessage(in_message);
        };
    }

    ~WebsocketServerFixture() {
        // Finished with the client
        websocketServer->stop();
        if (serverThread.joinable()) {
            serverThread.join();
        }
    }

    void startWebSocketServer() {
        // Start the client
        std::promise<bool> bReady;
        serverThread = std::thread([&]() {
            websocketServer->start([&bReady](uint16_t) { bReady.set_value(true); });
        });

        bReady.get_future().wait();
    }

    virtual void onWebsocketServerOpen(std::shared_ptr<TestWsServer::Connection> connection) {}
    virtual void onWebsocketServerMessage(std::shared_ptr<TestWsServer::InMessage> message) {}
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
