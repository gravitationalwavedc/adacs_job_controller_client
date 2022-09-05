//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H
#define ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H

#include "client_wss.hpp"
#include "../lib/Messaging/Message.h"
#include <shared_mutex>
#include <folly/concurrency/ConcurrentHashMap.h>
#include <folly/concurrency/UnboundedQueue.h>

using WsClient = SimpleWeb::SocketClient<SimpleWeb::WSS>;

class WebsocketInterface {
public:
    WebsocketInterface() {};
    WebsocketInterface(const std::string& token);

    static void SingletonFactory(const std::string& token);
    static auto Singleton() -> std::shared_ptr<WebsocketInterface>;

    void start();
    void join();
    void stop();

    // virtual here so that we can override this function for testing
    virtual void queueMessage(std::string source, const std::shared_ptr<std::vector<uint8_t>>& data, Message::Priority priority);

private:
    std::shared_ptr<WsClient> client;
    std::thread clientThread;
    std::string url;

    // Packet Queue is a:
    //  list of priorities - doesn't need any sync because it never changes
    //      -> map of sources - needs sync when adding/removing sources
    //          -> vector of packets - make this a MPSC queue

    // When the number of bytes in a packet of vectors exceeds some amount, a message should be sent that stops more
    // packets from being sent, when the vector then falls under some threshold

    // Track sources in the map, add them when required - delete them after some amount (1 minute?) of inactivity.

    // Send sources round robin, starting from the highest priority
    mutable std::shared_mutex mutex_;
    mutable std::mutex dataCVMutex;
    bool dataReady{};
    std::condition_variable dataCV;
    std::vector<folly::ConcurrentHashMap<std::string, std::shared_ptr<folly::UMPSCQueue<std::shared_ptr<std::vector<uint8_t>>, false>>>> queue;

#ifdef BUILD_TESTS
public:
    EXPOSE_PROPERTY_FOR_TESTING(url)
    static void setSingleton(std::shared_ptr<WebsocketInterface>);
#endif
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H
