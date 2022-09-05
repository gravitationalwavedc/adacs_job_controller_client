//
// Created by lewis on 9/4/22.
//

#include "../lib/GeneralUtils.h"
#include "WebsocketInterface.h"

static std::shared_ptr<WebsocketInterface> singleton;

WebsocketInterface::WebsocketInterface(const std::string& token) {
    if (singleton) {
        std::cerr << "WebsocketInterface singleton was already initialised!" << std::endl;
        std::terminate();
    }

    // Create the list of priorities in order
    for (auto i = static_cast<uint32_t>(Message::Priority::Highest); i <= static_cast<uint32_t>(Message::Priority::Lowest); i++) {
        queue.emplace_back();
    }

    auto config = readClientConfig();
    url = std::string{config["websocketEndpoint"]} + "?token=" + token;
    client = std::make_shared<WsClient>(url);

    client->on_open = [this](const std::shared_ptr<WsClient::Connection>& connection) {
        std::cout << "WS: Client connected to " << url << std::endl;
    };
}

void WebsocketInterface::start() {
    clientThread = std::thread([this]() {
        // Start server
        this->client->start();
    });
}

void WebsocketInterface::join() {
    if (clientThread.joinable()) {
        clientThread.join();
    }
}

void WebsocketInterface::stop() {
    client->stop();
    join();
}

void WebsocketInterface::SingletonFactory(const std::string& token) {
    if (singleton) {
        std::cerr << "WebsocketInterface singleton was already initialised!" << std::endl;
        std::terminate();
    }

    singleton = std::make_shared<WebsocketInterface>(token);
}

auto WebsocketInterface::Singleton() -> std::shared_ptr<WebsocketInterface> {
    if (!singleton) {
        std::cerr << "WebsocketInterface singleton was null!" << std::endl;
        std::terminate();
    }

    return singleton;
}

void WebsocketInterface::queueMessage(std::string source, const std::shared_ptr<std::vector<uint8_t>>& pData, Message::Priority priority) {
    // Get a pointer to the relevant map
    auto *pMap = &queue[priority];

    // Lock the access mutex to check if the source exists in the map
    {
        std::shared_lock<std::shared_mutex> lock(mutex_);

        // Make sure that this source exists in the map
        auto sQueue = std::make_shared<folly::UMPSCQueue<std::shared_ptr<std::vector<uint8_t>>, false>>();

        // Make sure that the source is in the map
        pMap->try_emplace(source, sQueue);

        // Write the data in the queue
        (*pMap)[source]->enqueue(pData);

        // Trigger the new data event to start sending
        this->dataReady = true;
        dataCV.notify_one();
    }
}

#ifdef BUILD_TESTS
void WebsocketInterface::setSingleton(std::shared_ptr<WebsocketInterface> newSingleton) {
    singleton = newSingleton;
}
#endif