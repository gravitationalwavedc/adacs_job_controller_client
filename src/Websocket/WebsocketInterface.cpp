//
// Created by lewis on 9/4/22.
//

#include "../lib/GeneralUtils.h"
#include "WebsocketInterface.h"
#include <utility>
#include "../Settings.h"
#include "../Core/MessageHandler.h"
#include "subprocess.hpp"
#include <boost/filesystem.hpp>

static std::shared_ptr<WebsocketInterface> singleton;

WebsocketInterface::WebsocketInterface(const std::string& token) : bServerReady(false) {
    if (singleton) {
        LOG(ERROR) << "WebsocketInterface singleton was already initialised!";
        abortApplication();
    }

    // Find the path to the system certificate store and set the SSL_CERT_FILE environment variable
    auto certPath = boost::filesystem::path(getOpensslCertPath()) / "cert.pem";
    if (!boost::filesystem::exists(certPath)) {
        LOG(ERROR) << "Generated OpenSSL cert file '" << certPath << "' doesn't exist. Please set it manually via the SSL_CERT_FILE environment variable";
    }
    setenv("SSL_CERT_FILE", certPath.c_str(), 0);

    // Create the list of priorities in order
    for (auto i = static_cast<uint32_t>(Message::Priority::Highest); i <= static_cast<uint32_t>(Message::Priority::Lowest); i++) {
        queue.emplace_back(std::make_shared<folly::ConcurrentHashMap<std::string, std::shared_ptr<folly::UMPSCQueue<sDataItem, false>>>>());
    }

    // Set up the db requests
    dbRequestCounter = 0;

    auto config = readClientConfig();
    url = std::string{config["websocketEndpoint"]} + "?token=" + token;
    client = std::make_shared<WsClient>(url);

    client->on_error = [&](auto, auto error) {
        LOG(ERROR) << "WS: Error with connection to " << url;
        LOG(ERROR) << error.message();
        abortApplication();
    };

    client->on_open = [&](const std::shared_ptr<WsClient::Connection>& connection) {
        LOG(INFO) << "WS: Client connected to " << url;
        pConnection = connection;
    };

    client->on_close = [&](const std::shared_ptr<WsClient::Connection>&, int, const std::string &) {
        LOG(INFO) << "WS: Client connection closed to " << url;
        pConnection = nullptr;
        closePromise.set_value();
    };

    client->on_pong = [&](const std::shared_ptr<WsClient::Connection>&) {
        handlePong();
    };

    client->on_message = [&](const std::shared_ptr<WsClient::Connection>&, std::shared_ptr<WsClient::InMessage> inMessage) {
        auto stringData = inMessage->string();
        auto message = std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end()));
        handleMessage(message);
    };
}

WebsocketInterface::~WebsocketInterface() {
    stop();
}

void WebsocketInterface::start() {
    std::promise<void> bReady;
    clientThread = std::thread([&]() {
        // Start server
        client->start([&]() { bReady.set_value(); });
    });

    bReady.get_future().wait();
}

void WebsocketInterface::serverReady() {
    LOG(INFO) << "WS: Server ready - starting threads";
#ifndef BUILD_TESTS
    // Start the scheduler thread
    schedulerThread = std::thread([this] {
        this->run();
    });

    // Start the prune thread
    pruneThread = std::thread([this] {
        this->pruneSources();
    });

    // Start the ping thread
    pingThread = std::thread([this] {
        this->runPings();
    });
#endif
}

void WebsocketInterface::join() {
    if (clientThread.joinable()) {
        clientThread.join();
    }
}

void WebsocketInterface::stop() {
    if (client) {
        if (pConnection) {
            closePromise = std::promise<void>();
            pConnection->send_close(1000);
            closePromise.get_future().wait_for(std::chrono::milliseconds(100));
        }
        client->stop();
    }

    join();
}

void WebsocketInterface::SingletonFactory(const std::string& token) {
    if (singleton) {
        LOG(ERROR) << "WebsocketInterface singleton was already initialised!";
        abortApplication();
    }

    singleton = std::make_shared<WebsocketInterface>(token);
}

auto WebsocketInterface::Singleton() -> std::shared_ptr<WebsocketInterface> {
    if (!singleton) {
        LOG(ERROR) << "WebsocketInterface singleton was null!";
        abortApplication();
    }

    return singleton;
}

void WebsocketInterface::queueMessage(std::string source, const std::shared_ptr<std::vector<uint8_t>>& pData, Message::Priority priority, std::function<void()> callback) {
    // Get a pointer to the relevant map
    auto *pMap = &queue[priority];

    // Lock the access mutex to check if the source exists in the map
    {
        std::shared_lock<std::shared_mutex> lock(mutex_);

        // Make sure that this source exists in the map
        auto sQueue = std::make_shared<folly::UMPSCQueue<sDataItem, false>>();

        // Make sure that the source is in the map
        pMap->get()->try_emplace(source, sQueue);

        // Write the data in the queue
        (*pMap->get())[source]->enqueue({pData, std::move(callback)});

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

#ifndef BUILD_TESTS
[[noreturn]] void WebsocketInterface::pruneSources() {
    // Iterate forever
    while (true) {
        // Wait 1 minute until the next prune
        std::this_thread::sleep_for(std::chrono::seconds(QUEUE_SOURCE_PRUNE_SECONDS));
#else

void WebsocketInterface::pruneSources() {
#endif
    // Acquire the exclusive lock to prevent more data being pushed on while we are pruning
    {
        std::unique_lock<std::shared_mutex> lock(mutex_);

        // Iterate over the priorities
        for (auto &priority : queue) {
            // Get a pointer to the relevant map
            auto *pMap = &priority;

            // Iterate over the map
            for (auto iter = pMap->get()->begin(); iter != pMap->get()->end();) {
                // Check if the vector for this source is empty
                if ((*iter).second->empty()) {
                    // Remove this source from the map and continue
                    iter = pMap->get()->erase(iter);
                    continue;
                }
                // Manually increment the iterator
                ++iter;
            }
        }
    }
#ifndef BUILD_TESTS
    }
#endif
}

#ifndef BUILD_TESTS
[[noreturn]] void WebsocketInterface::run() { // NOLINT(readability-function-cognitive-complexity)
    // Iterate forever
    while (true) {
#else

void WebsocketInterface::run() { // NOLINT(readability-function-cognitive-complexity)
#endif
    {
        std::unique_lock<std::mutex> lock(dataCVMutex);

        // Wait for data to be ready to send
        dataCV.wait(lock, [this] { return this->dataReady; });

        // Reset the condition
        this->dataReady = false;
    }

    reset:

    // Iterate over the priorities
    for (auto priority = queue.begin(); priority != queue.end(); priority++) {

        // Get a pointer to the relevant map
        auto *pMap = &(*priority);

        // Get the current priority
        auto currentPriority = priority - queue.begin();

        // While there is still data for this priority, send it
        bool hadData = false;
        do {
            hadData = false;

            std::shared_lock<std::shared_mutex> lock(mutex_);
            // Iterate over the map
            for (auto iter = pMap->get()->begin(); iter != pMap->get()->end(); ++iter) {
                // Check if the vector for this source is empty
                if (!(*iter).second->empty()) {

                    // Pop the next item from the queue
                    auto data = (*iter).second->try_dequeue();

                    try {
                        // data should never be null as we're checking for empty
                        if (data) {
                            // Convert the message
                            auto outMessage = std::make_shared<WsClient::OutMessage>((*data).data->size());
                            std::copy((*data).data->begin(), (*data).data->end(), std::ostream_iterator<uint8_t>(*outMessage));

                            // Send the message on the websocket
                            if (pConnection != nullptr) {
                                pConnection->send(
                                        outMessage,
                                        [this](const SimpleWeb::error_code &errorCode) {
                                            // Kill the connection only if the error was not indicating success
                                            if (!errorCode) {
                                                return;
                                            }

                                            pConnection->close();
                                            pConnection = nullptr;

                                            reportWebsocketError(errorCode);

                                            abortApplication();
                                        },
                                        // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
                                        130
                                );

                                (*data).callback();
                            }
                        }
                    } catch (std::exception& exception) {
                        LOG(ERROR) << "Exception: " __FILE__ ":" << __LINE__ << " > " << exception.what();
                    }

                    // Data existed
                    hadData = true;
                }
            }

            // Check if there is higher priority data to send
            if (doesHigherPriorityDataExist(currentPriority)) {
                // Yes, so start the entire send process again
                goto reset; // NOLINT(cppcoreguidelines-avoid-goto,hicpp-avoid-goto)
            }

            // Higher priority data does not exist, so keep sending data from this priority
        } while (hadData);
    }
#ifndef BUILD_TESTS
    }
#endif
}

auto WebsocketInterface::doesHigherPriorityDataExist(uint64_t maxPriority) -> bool {
    for (auto priority = queue.begin(); priority != queue.end(); priority++) {
        // Get a pointer to the relevant map
        auto *pMap = &(*priority);

        // Check if the current priority is greater or equal to max priority and return false if not.
        auto currentPriority = priority - queue.begin();
        if (currentPriority >= maxPriority) {
            return false;
        }

        // Iterate over the map
        for (auto iter = pMap->get()->begin(); iter != pMap->get()->end();) {
            // Check if the vector for this source is empty
            if (!(*iter).second->empty()) {
                // It'iter not empty so data does exist
                return true;
            }

            // Increment the iterator
            ++iter;
        }
    }

    return false;
}

void WebsocketInterface::reportWebsocketError(const SimpleWeb::error_code &errorCode) {
    // Log this
    LOG(ERROR) << "WS: Error in connection. "
              << "Error: " << errorCode << ", error message: " << errorCode.message();
}

void WebsocketInterface::handlePong() {
    // Update the ping timer
    pongTimestamp = std::chrono::system_clock::now();

    // Report the latency
    auto latency = pongTimestamp - pingTimestamp;

    LOG(INFO) << "WS: Had " << std::chrono::duration_cast<std::chrono::milliseconds>(latency).count()
    << "ms latency with the server.";
}

[[noreturn]] void WebsocketInterface::runPings() {
    while (true) {
        checkPings();

        // Wait PING_INTERVAL_SECONDS to check again
        std::this_thread::sleep_for(std::chrono::seconds(PING_INTERVAL_SECONDS));
    }
}

void WebsocketInterface::checkPings() {
    // Check for any websocket pings that didn't pong within PING_INTERVAL_SECONDS, and terminate if so

    std::chrono::time_point<std::chrono::system_clock> zeroTime = {};
    if (pingTimestamp != zeroTime && pongTimestamp == zeroTime) {
        LOG(ERROR) << "WS: Error in connection with " << url << ". "
                  << "Error: Websocket timed out waiting for ping.";

        abortApplication();
    }

    // Send a fresh ping to the server
    // Update the ping timestamp
    pingTimestamp = std::chrono::system_clock::now();
    pongTimestamp = {};

    // Send a ping to the client
    // See https://www.rfc-editor.org/rfc/rfc6455#section-5.2 for the ping opcode 137
    pConnection->send(
            "",
            [&](const SimpleWeb::error_code &errorCode){
                // Kill the server only if the error was not indicating success
                if (!errorCode){
                    return;
                }

                reportWebsocketError(errorCode);
                abortApplication();
            },
            // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers, readability-magic-numbers)
            137
    );
}

std::string WebsocketInterface::getOpensslCertPath() {
    auto proc = subprocess::Popen(
            {"openssl", "version", "-d"},
            subprocess::output{subprocess::PIPE},
            subprocess::error{subprocess::PIPE},
            subprocess::shell{false}
    );

    // Get the output from the process
    auto communication = proc.communicate();
    auto obuf = communication.first;
    auto ebuf = communication.second;

    std::string sOut(obuf.buf.begin(), obuf.buf.end());
    std::string sErr(ebuf.buf.begin(), ebuf.buf.end());

    if (proc.retcode() != 0) {
        LOG(ERROR) << "Error fetching openssl certificate directory";
        LOG(ERROR) << "stdout: " << std::endl << sOut;
        LOG(ERROR) << "stderr: " << std::endl << sErr;
        abortApplication();
    }

    // Split the output and find the "OPENSSLDIR" line
    std::string infoLine;
    for (const auto& line : splitString(sOut, "\n")) {
        if (line.starts_with("OPENSSLDIR")) {
            infoLine = {line};
            break;
        }
    }

    if (infoLine.empty()) {
        LOG(ERROR) << "Error fetching openssl certificate directory";
        LOG(ERROR) << "stdout: " << std::endl << sOut;
        LOG(ERROR) << "stderr: " << std::endl << sErr;
        abortApplication();
    }

    // Get the path to the certificates
    auto bits = splitString(infoLine, " ");
    if (bits.size() != 2) {
        LOG(ERROR) << "Error fetching openssl certificate directory";
        LOG(ERROR) << "stdout: " << std::endl << sOut;
        LOG(ERROR) << "stderr: " << std::endl << sErr;
        abortApplication();
    }

    while (bits[1].find("\"") != std::string::npos) {
        bits[1].replace(bits[1].find("\""), 1, "");
    }

    return bits[1];
}

auto WebsocketInterface::generateDbRequestId() -> uint64_t {
    // Atomically increment the request counter and get the new value
    auto result = dbRequestCounter++;

    // Create a new promise for this request id
    dbRequestPromises.emplace(result, std::promise<std::shared_ptr<Message>>{});

    return result;
}

auto WebsocketInterface::getDbResponse(uint64_t dbRequestId) -> std::shared_ptr<Message> {
    // Wait for the future to be set
    auto result = dbRequestPromises[dbRequestId].get_future().get();

    // We're done with the dbRequestId now, so remove it from the promises map
    dbRequestPromises.erase(dbRequestId);

    return result;
}

void WebsocketInterface::setDbRequestResponse(const std::shared_ptr<Message>& message) {
    // The first ulong is always the db request id
    auto dbRequestId = message->pop_ulong();

    if (!dbRequestPromises.contains(dbRequestId)) {
        LOG(WARNING) << "Got unexpected DB Request ID response " << dbRequestId;
        return;
    }

    // Fulfil the promise
    dbRequestPromises[dbRequestId].set_value(message);
}
