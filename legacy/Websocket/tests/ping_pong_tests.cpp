//
// Created by lewis on 8/17/22.
//

#include "../../Tests/fixtures/AbortHelperFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"

struct PingPongTestDataFixture : public WebsocketServerFixture, public AbortHelperFixture {
    bool bReceivedPing = false;
    std::chrono::time_point<std::chrono::system_clock> zeroTime = {};
    std::string token;

    PingPongTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    ~PingPongTestDataFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerPing() override {
        bReceivedPing = true;
    };

    void runCheckPings() {
        bReceivedPing = false;

        WebsocketInterface::Singleton()->callcheckPings();

        while (!bReceivedPing && !bServerConnectionClosed) {
            // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }

        // Wait a moment for the pong to be processed
        // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }
};

BOOST_FIXTURE_TEST_SUITE(ping_pong_test_suite, PingPongTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_sane_initial_values) {
        // Check that the default setup for the pings is correct
        BOOST_CHECK_MESSAGE(*WebsocketInterface::Singleton()->getpingTimestamp() == zeroTime, "pingTimestamp was not zero when it should have been");
        BOOST_CHECK_MESSAGE(*WebsocketInterface::Singleton()->getpongTimestamp() == zeroTime, "pongTimestamp was not zero when it should have been");
    }

    BOOST_AUTO_TEST_CASE(test_checkPings_send_ping_success) {
        // First check that when checkPings is called for the first time after a cluster has connected
        // that a ping is sent, and a pong received
        runCheckPings();

        // Check that neither ping or pong timestamp is zero
        BOOST_CHECK_MESSAGE(*WebsocketInterface::Singleton()->getpingTimestamp() != zeroTime, "pingTimestamp was zero when it should not have been");
        BOOST_CHECK_MESSAGE(*WebsocketInterface::Singleton()->getpongTimestamp() != zeroTime, "pongTimestamp was zero when it should not have been");

        auto previousPingTimestamp = *WebsocketInterface::Singleton()->getpingTimestamp();
        auto previousPongTimestamp = *WebsocketInterface::Singleton()->getpongTimestamp();

        // Run the ping pong again, the new ping/pong timestamps should be greater than the previous ones
        runCheckPings();

        // Check that neither ping or pong timestamp is zero
        BOOST_CHECK_MESSAGE(*WebsocketInterface::Singleton()->getpingTimestamp() > previousPingTimestamp, "pingTimestamp was not greater than the previous ping timestamp when it should not have been");
        BOOST_CHECK_MESSAGE(*WebsocketInterface::Singleton()->getpongTimestamp() > previousPongTimestamp, "pongTimestamp was not greater than the previous pong timestamp when it should not have been");
    }

    BOOST_AUTO_TEST_CASE(test_checkPings_handle_zero_time) {
        // If checkPings is called, and the pongTimestamp is zero, then the connection should be disconnected. This
        // case indicates that the remote end never responded to the ping, or did not respond to the ping in
        // a timely manner (indicating a communication problem)
        runCheckPings();

        // Set the pongTimeout back to zero
        *WebsocketInterface::Singleton()->getpongTimestamp() = zeroTime;

        // Application should not be aborted yet
        BOOST_CHECK_EQUAL(checkAborted(), false);

        // Running checkPings should now terminate the entire application
        BOOST_CHECK_THROW(runCheckPings(), std::runtime_error);

        // Check that the application aborted
        BOOST_CHECK_EQUAL(checkAborted(), true);
    }
BOOST_AUTO_TEST_SUITE_END()
  