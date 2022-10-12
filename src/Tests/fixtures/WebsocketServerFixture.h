//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
#define ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H

#include "../../Lib/GeneralUtils.h"
#include "../../Lib/Messaging/Message.h"
#include "../../Settings.h"
#include "../../Tests/utils.h"
#include "../../Websocket/WebsocketInterface.h"
#include "../utils.h"
#include "DatabaseFixture.h"
#include "JsonConfigFixture.h"
#include <boost/test/unit_test.hpp>
#include <fstream>

class WebsocketServerFixture : public JsonConfigFixture, public DatabaseFixture {
public:
    std::shared_ptr<TestWsServer> websocketServer;
    std::thread serverThread;
    bool clientRunning = true;
    std::thread clientThread;
    std::promise<void> websocketServerConnectionPromise;
    std::shared_ptr<TestWsServer::Connection> pWebsocketServerConnection;
    bool bServerConnectionClosed = true;

    explicit WebsocketServerFixture(bool run = true) {
        websocketServer = std::make_shared<TestWsServer>();
        websocketServer->config.port = TEST_SERVER_PORT;

        websocketServer->endpoint["^(.*?)$"].on_open = [&, run](auto connection) {
            bServerConnectionClosed = false;
            pWebsocketServerConnection = connection;
            onWebsocketServerOpen(connection);

            websocketServerConnectionPromise.set_value();

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

        websocketServer->endpoint["^(.*?)$"].on_message = [&](auto connection, auto in_message) {
            handleWebsocketServerMessage(connection, in_message);
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

    virtual ~WebsocketServerFixture() {
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

    void handleWebsocketServerMessage(auto connection, auto in_message) {
        auto stringData = in_message->string();
        auto message = std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end()));

        if (maybeHandleDbMessage(connection, message)) {
            // Nothing to do if the message has been handled
            return;
        }

        onWebsocketServerMessage(std::make_shared<Message>(**message->getdata()), connection);
    }

    virtual void onWebsocketServerOpen(const std::shared_ptr<TestWsServer::Connection>& connection) {}
    virtual void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& connection) {}
    virtual void onWebsocketServerPing() {}
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
