//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H
#define ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H

#include "../Lib/Messaging/Message.h"
#include "client_wss.hpp"
#include <folly/concurrency/ConcurrentHashMap.h>
#include <folly/concurrency/UnboundedQueue.h>
#include <shared_mutex>

#ifndef BUILD_TESTS
using WsClient = SimpleWeb::SocketClient<SimpleWeb::WSS>;
#else
using WsClient = SimpleWeb::SocketClient<SimpleWeb::WS>;
#endif

class WebsocketInterface {
public:
    WebsocketInterface() = default;
    explicit WebsocketInterface(const std::string& token);
    virtual ~WebsocketInterface(); // NOLINT(bugprone-exception-escape)

    static void SingletonFactory(const std::string& token);
    static auto Singleton() -> std::shared_ptr<WebsocketInterface>;

    auto start() -> void;
    void join();
    void stop();

    // virtual here so that we can override this function for testing
    virtual void queueMessage(std::string source, const std::shared_ptr<std::vector<uint8_t>>& data, Message::Priority priority, std::function<void()> callback);

    void serverReady();
    auto isServerReady() const -> bool {
        return bServerReady;
    }

    // Database helpers
    auto generateDbRequestId() -> uint64_t;
    auto getDbResponse(uint64_t requestId) -> std::shared_ptr<Message>;
    void setDbRequestResponse(const std::shared_ptr<Message>& message);
private:
    std::shared_ptr<WsClient> client;
    std::shared_ptr<WsClient::Connection> pConnection = nullptr;
    std::thread clientThread;
    std::string url;
    std::promise<void> closePromise;
    bool bServerReady = false;

#ifndef BUILD_TESTS
    [[noreturn]] void run();
    [[noreturn]] void pruneSources();
#else
    void run();
    void pruneSources();
#endif
    [[noreturn]] void runPings();
    void checkPings();
    void handlePong();

    auto doesHigherPriorityDataExist(uint64_t maxPriority) -> bool;
    static void reportWebsocketError(const SimpleWeb::error_code &errorCode);

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
    struct sDataItem {
        std::shared_ptr<std::vector<uint8_t>> data = nullptr;
        std::function<void()> callback;
    };
    std::vector<std::shared_ptr<folly::ConcurrentHashMap<std::string, std::shared_ptr<folly::UMPSCQueue<sDataItem, false>>>>> queue;

    std::chrono::time_point<std::chrono::system_clock> pingTimestamp;
    std::chrono::time_point<std::chrono::system_clock> pongTimestamp;

    std::thread pingThread;
    std::thread schedulerThread;
    std::thread pruneThread;

    std::shared_mutex requestAccessMutex;
    std::map<uint64_t, std::promise<std::shared_ptr<Message>>> dbRequestPromises;
    std::atomic<uint64_t> dbRequestCounter;

#ifdef BUILD_TESTS
public:
    EXPOSE_PROPERTY_FOR_TESTING(url)
    EXPOSE_PROPERTY_FOR_TESTING(queue)
    EXPOSE_PROPERTY_FOR_TESTING_READONLY(dataReady)
    EXPOSE_PROPERTY_FOR_TESTING_READONLY(pingTimestamp)
    EXPOSE_PROPERTY_FOR_TESTING_READONLY(pongTimestamp)
    EXPOSE_PROPERTY_FOR_TESTING_READONLY(pConnection)

    EXPOSE_FUNCTION_FOR_TESTING(pruneSources)
    EXPOSE_FUNCTION_FOR_TESTING(run)
    EXPOSE_FUNCTION_FOR_TESTING(checkPings)
    EXPOSE_FUNCTION_FOR_TESTING_ONE_PARAM(doesHigherPriorityDataExist, uint64_t)

    static void setSingleton(const std::shared_ptr<WebsocketInterface>& newSingleton);
#endif
};


#endif //ADACS_JOB_CLIENT_WEBSOCKETINTERFACE_H
