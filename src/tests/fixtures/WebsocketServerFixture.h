//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
#define ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H

#include "../../Settings.h"
#include "../../tests/utils.h"
#include "../../lib/GeneralUtils.h"
#include "../utils.h"
#include "./certs/test.crt.h"
#include "./certs/test.key.h"
#include <boost/test/unit_test.hpp>
#include <fstream>
#include "JsonConfigFixture.h"

static constexpr char* TEST_CERT_FILENAME = "test.crt";
static constexpr char* TEST_KEY_FILENAME = "test.key";

class WebsocketServerFixture : public JsonConfigFixture {
public:
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::shared_ptr<TestWsServer> websocketServer;
    std::thread serverThread;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    WebsocketServerFixture() {
        writeCertFiles();
        websocketServer = std::make_shared<TestWsServer>(TEST_CERT_FILENAME, TEST_KEY_FILENAME);
        websocketServer->config.address = TEST_SERVER_HOST;
        websocketServer->config.port = TEST_SERVER_PORT;
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
        serverThread = std::thread([&]() {
            websocketServer->start();
        });

        BOOST_CHECK_EQUAL(acceptingConnections(8000), true);
    }

//    void onOpen() {
//         Tell the client that we are ready
//        Message msg(SERVER_READY, Message::Priority::Highest, SYSTEM_SOURCE);
//        msg.send(cluster);
//    }

private:
    static void writeCertFiles() {
        std::ofstream crt;
        crt.open(TEST_CERT_FILENAME, std::ios::out);
        crt.write(reinterpret_cast<char *>(&test_crt), test_crt_len);
        crt.close();

        std::ofstream key;
        key.open(TEST_KEY_FILENAME, std::ios::out);
        key.write(reinterpret_cast<char *>(&test_key), test_key_len);
        key.close();
    }
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETSERVERFIXTURE_H
