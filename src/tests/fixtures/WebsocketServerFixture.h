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
#include "../../lib/Messaging/Message.h"
#include "../../Websocket/WebsocketInterface.h"

class WebsocketServerFixture : public JsonConfigFixture {
public:
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::shared_ptr<TestWsServer> websocketServer;
    std::thread serverThread;
    bool clientRunning = true;
    std::thread clientThread;
    std::promise<void> websocketServerConnectionPromise;
    std::shared_ptr<TestWsServer::Connection> pWebsocketServerConnection;
    bool bServerConnectionClosed = true;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    explicit WebsocketServerFixture(bool run = true) {
        websocketServer = std::make_shared<TestWsServer>();
        websocketServer->config.port = TEST_SERVER_PORT;

        websocketServer->endpoint["^(.*?)$"].on_open = [&, run](auto connection) {
            bServerConnectionClosed = false;
            websocketServerConnectionPromise.set_value();
            pWebsocketServerConnection = connection;
            onWebsocketServerOpen(connection);

            Message msg(SERVER_READY, Message::Priority::Highest, SYSTEM_SOURCE);
            msg.send(connection);

            if (run) {
                // Start the scheduler thread
                clientThread = std::thread([&] {
                    while (clientRunning) {
                        WebsocketInterface::Singleton()->callrun();
                        WebsocketInterface::Singleton()->callpruneSources();
                    }
                });
            }
        };

        websocketServer->endpoint["^(.*?)$"].on_message = [&]([[maybe_unused]] auto connection, auto in_message) {
            onWebsocketServerMessage(in_message);
        };

        websocketServer->endpoint["^(.*?)$"].on_ping = [&]([[maybe_unused]] auto connection) {
            onWebsocketServerPing();
        };

        websocketServer->endpoint["^(.*?)$"].on_close = [&](auto, auto, auto) {
            bServerConnectionClosed = true;
        };

        websocketServer->endpoint["^(.*?)$"].on_error = [&](auto, auto) {
            bServerConnectionClosed = true;
        };
    }

    ~WebsocketServerFixture() {
        // Shut down the client, send any message at all to trigger the internal run event and escape the loop
        clientRunning = false;
        Message msg(SERVER_READY, Message::Priority::Highest, SYSTEM_SOURCE);
        msg.send();
        if (clientThread.joinable()) {
            clientThread.join();
        }

        // Finished with the client
        websocketServer->stop();
        if (serverThread.joinable()) {
            serverThread.join();
        }

        // Really wait until the server has shut down
        while (acceptingConnections(TEST_SERVER_PORT)) {}
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
    virtual void onWebsocketServerPing() {}
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
