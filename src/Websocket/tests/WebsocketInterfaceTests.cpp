//
// Created by lewis on 9/5/22.
//

#include <boost/test/unit_test.hpp>
#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../WebsocketInterface.h"

struct WebsocketInterfaceFixture : public WebsocketServerFixture {
public:
    std::string token;
    std::vector<std::vector<uint8_t>> receivedMessages;

    WebsocketInterfaceFixture() : WebsocketServerFixture(false) {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();
    }

    virtual ~WebsocketInterfaceFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void startClient() {
        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    void onWebsocketServerMessage(std::shared_ptr<TestWsServer::InMessage> inMessage) override {
        auto stringData = inMessage->string();
        auto data = std::vector<uint8_t>(stringData.begin(), stringData.end());
        auto message = Message(data);

        receivedMessages.emplace_back(message.pop_bytes());
    }
};

BOOST_FIXTURE_TEST_SUITE(websocket_interface_tests, WebsocketInterfaceFixture)
    BOOST_AUTO_TEST_CASE(test_constructor) {
        BOOST_CHECK_EQUAL(*WebsocketInterface::Singleton()->geturl(), std::string{TEST_SERVER_URL} + "?token=" + token);

        // Check that the right number of queue levels are created (+1 because 0 is a priority level itself)
        BOOST_CHECK_EQUAL(
                WebsocketInterface::Singleton()->getqueue()->size(),
                static_cast<uint32_t>(Message::Priority::Lowest) - static_cast<uint32_t>(Message::Priority::Highest) + 1
        );
    }

    BOOST_AUTO_TEST_CASE(test_queueMessage) {
        // Check the source doesn't exist
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), true);

        auto s1_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s1", s1_d1, Message::Priority::Highest);

        auto s2_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s2", s2_d1, Message::Priority::Lowest);

        auto s3_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s3", s3_d1, Message::Priority::Lowest);

        // s1 should only exist in the highest priority queue
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), false);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), true);

        // s2 should only exist in the lowest priority queue
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s2") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->find("s2") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s2") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), false);

        // s3 should only exist in the lowest priority queue
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s3") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->find("s3") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s3") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), false);

        auto find_s1 = (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1");
        // s1 should have been put in the queue exactly once
        BOOST_CHECK_EQUAL(find_s1->second->size(), 1);
        // The found s1 should exactly equal s1_d1
        BOOST_CHECK_EQUAL_COLLECTIONS((*find_s1->second->try_peek()).data->begin(), (*find_s1->second->try_peek()).data->end(),
                                      s1_d1->begin(), s1_d1->end());

        auto find_s2 = (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s2");
        // s2 should have been put in the queue exactly once
        BOOST_CHECK_EQUAL(find_s2->second->size(), 1);
        // The found s2 should exactly equal s2_d1
        BOOST_CHECK_EQUAL_COLLECTIONS((*find_s2->second->try_peek()).data->begin(), (*find_s2->second->try_peek()).data->end(),
                                      s2_d1->begin(), s2_d1->end());

        auto find_s3 = (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s3");
        // s2 should have been put in the queue exactly once
        BOOST_CHECK_EQUAL(find_s3->second->size(), 1);
        // The found s2 should exactly equal s2_d1
        BOOST_CHECK_EQUAL_COLLECTIONS((*find_s3->second->try_peek()).data->begin(), (*find_s3->second->try_peek()).data->end(),
                                      s3_d1->begin(), s3_d1->end());

        auto s1_d2 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s1", s1_d2, Message::Priority::Highest);
        // s1 should 2 items
        BOOST_CHECK_EQUAL(find_s1->second->size(), 2);

        auto s1_d3 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s1", s1_d3, Message::Priority::Highest);

        // s1 should have 3 items
        BOOST_CHECK_EQUAL(find_s1->second->size(), 3);

        // Test dequeuing gives the correct results
        auto data = find_s1->second->dequeue();
        // d should be the same reference as s1_d1
        BOOST_CHECK_EQUAL(data.data == s1_d1, true);
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s1_d1->begin(), s1_d1->end());

        data = find_s1->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s1_d2->begin(), s1_d2->end());

        data = find_s1->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s1_d3->begin(), s1_d3->end());

        auto s2_d2 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s2", s2_d2, Message::Priority::Lowest);
        // s2 should 2 items
        BOOST_CHECK_EQUAL(find_s2->second->size(), 2);

        auto s2_d3 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s2", s2_d3, Message::Priority::Lowest);

        // s2 should have 3 items
        BOOST_CHECK_EQUAL(find_s2->second->size(), 3);

        auto s3_d2 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s3", s3_d2, Message::Priority::Lowest);
        // s3 should 2 items
        BOOST_CHECK_EQUAL(find_s3->second->size(), 2);

        auto s3_d3 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s3", s3_d3, Message::Priority::Lowest);

        // s3 should have 3 items
        BOOST_CHECK_EQUAL(find_s3->second->size(), 3);

        // Test dequeuing gives the correct results
        data = find_s2->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s2_d1->begin(), s2_d1->end());

        data = find_s2->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s2_d2->begin(), s2_d2->end());

        data = find_s2->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s2_d3->begin(), s2_d3->end());

        data = find_s3->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s3_d1->begin(), s3_d1->end());

        data = find_s3->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s3_d2->begin(), s3_d2->end());

        data = find_s3->second->dequeue();
        BOOST_CHECK_EQUAL_COLLECTIONS(data.data->begin(), data.data->end(), s3_d3->begin(), s3_d3->end());

        // Check that after all data has been dequeued, that s1, s2, and s3 queues are empty
        BOOST_CHECK_EQUAL(find_s1->second->empty(), true);
        BOOST_CHECK_EQUAL(find_s2->second->empty(), true);
        BOOST_CHECK_EQUAL(find_s3->second->empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_pruneSources) {
        // Create several sources and insert data in the queue
        auto s1_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s1", s1_d1, Message::Priority::Highest);

        auto s2_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s2", s2_d1, Message::Priority::Lowest);

        auto s3_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s3", s3_d1, Message::Priority::Lowest);

        // Pruning the sources should not perform any action since all sources have one item in the queue
        WebsocketInterface::Singleton()->callpruneSources();

        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), false);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s2") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), false);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s3") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), false);

        // Dequeue an item from s2, which will leave s2 with 0 items
        (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s2")->second->dequeue();

        // Now pruning the sources should remove s2, but not s1 or s3
        WebsocketInterface::Singleton()->callpruneSources();

        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), false);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s2") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s3") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), false);

        // Dequeue the remaining items from s1 and s3
        (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1")->second->dequeue();
        (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s3")->second->dequeue();

        // Now pruning the sources should remove both s1 and s3
        WebsocketInterface::Singleton()->callpruneSources();

        // There should now be no items left in the queue
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s2") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s3") ==
                          (*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->end(), true);

        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->empty(), true);
        BOOST_CHECK_EQUAL((*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_run) {
        startClient();

        bool cbCalled = false;

        // Create several sources and insert data in the queue
        auto s1_d1 = generateRandomData(randomInt(0, 255));
        auto msg = Message(-1, Message::Priority::Highest, "s1", [&] { cbCalled = true; });
        msg.push_bytes(*s1_d1);
        msg.send();

        auto s1_d2 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Highest, "s1");
        msg.push_bytes(*s1_d2);
        msg.send();

        auto s2_d1 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Highest, "s2");
        msg.push_bytes(*s2_d1);
        msg.send();

        auto s3_d1 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s3");
        msg.push_bytes(*s3_d1);
        msg.send();

        auto s3_d2 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s3");
        msg.push_bytes(*s3_d2);
        msg.send();

        auto s3_d3 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s3");
        msg.push_bytes(*s3_d3);
        msg.send();

        auto s3_d4 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s3");
        msg.push_bytes(*s3_d4);
        msg.send();

        auto s4_d1 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s4");
        msg.push_bytes(*s4_d1);
        msg.send();

        auto s4_d2 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s4");
        msg.push_bytes(*s4_d2);
        msg.send();

        auto s5_d1 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s5");
        msg.push_bytes(*s5_d1);
        msg.send();

        auto s5_d2 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Lowest, "s5");
        msg.push_bytes(*s5_d2);
        msg.send();

        auto s6_d1 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Medium, "s6");
        msg.push_bytes(*s6_d1);
        msg.send();

        auto s6_d2 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Medium, "s6");
        msg.push_bytes(*s6_d2);
        msg.send();

        auto s6_d3 = generateRandomData(randomInt(0, 255));
        msg = Message(-1, Message::Priority::Medium, "s6");
        msg.push_bytes(*s6_d3);
        msg.send();

        // Callback shouldn't be called yet
        BOOST_CHECK_EQUAL(cbCalled, false);

        *WebsocketInterface::Singleton()->getdataReady() = true;
        WebsocketInterface::Singleton()->callrun();

        // Wait for the messages to be sent
        while (receivedMessages.size() < 14) {
            // NOLINTNEXTLINE(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }

        // Callback should now be called
        BOOST_CHECK_EQUAL(cbCalled, true);

        // Check that the data sent was in priority/source order
        // The following order is deterministic - but sensitive.
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[0].begin(), receivedMessages[0].end(), s2_d1->begin(),
                                      s2_d1->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[1].begin(), receivedMessages[1].end(), s1_d1->begin(),
                                      s1_d1->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[2].begin(), receivedMessages[2].end(), s1_d2->begin(),
                                      s1_d2->end());

        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[3].begin(), receivedMessages[3].end(), s6_d1->begin(),
                                      s6_d1->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[4].begin(), receivedMessages[4].end(), s6_d2->begin(),
                                      s6_d2->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[5].begin(), receivedMessages[5].end(), s6_d3->begin(),
                                      s6_d3->end());

        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[6].begin(), receivedMessages[6].end(), s4_d1->begin(),
                                      s4_d1->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[7].begin(), receivedMessages[7].end(), s5_d1->begin(),
                                      s5_d1->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[8].begin(), receivedMessages[8].end(), s3_d1->begin(),
                                      s3_d1->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[9].begin(), receivedMessages[9].end(), s4_d2->begin(),
                                      s4_d2->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[10].begin(), receivedMessages[10].end(), s5_d2->begin(),
                                      s5_d2->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[11].begin(), receivedMessages[11].end(), s3_d2->begin(),
                                      s3_d2->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[12].begin(), receivedMessages[12].end(), s3_d3->begin(),
                                      s3_d3->end());
        BOOST_CHECK_EQUAL_COLLECTIONS(receivedMessages[13].begin(), receivedMessages[13].end(), s3_d4->begin(),
                                      s3_d4->end());
    }

    BOOST_AUTO_TEST_CASE(test_doesHigherPriorityDataExist) {
        // There should be no higher priority data if there is no data
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), false);

        // Insert some data
        auto s4_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s4", s4_d1, Message::Priority::Lowest);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Medium), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), false);

        auto s3_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s3", s3_d1, Message::Priority::Medium);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Medium), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), true);

        auto s2_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s2", s2_d1, Message::Priority::Highest);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Medium), true);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), true);

        auto s1_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s1", s1_d1, Message::Priority::Highest);
        auto s0_d1 = generateRandomData(randomInt(0, 255));
        WebsocketInterface::Singleton()->queueMessage("s0", s0_d1, Message::Priority::Highest);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Medium), true);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), true);

        // Clear all data from s2, s1 and s0
        *(*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s2")->second->try_dequeue();
        *(*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s1")->second->try_dequeue();
        *(*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Highest]->find("s0")->second->try_dequeue();
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Medium), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), true);

        // Clear data from s3 and s4
        *(*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Medium]->find("s3")->second->try_dequeue();
        *(*WebsocketInterface::Singleton()->getqueue())[Message::Priority::Lowest]->find("s4")->second->try_dequeue();
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Highest), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Medium), false);
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest), false);

        // Testing a non-standard priority that has a value greater than Lowest should now result in false
        BOOST_CHECK_EQUAL(WebsocketInterface::Singleton()->calldoesHigherPriorityDataExist((uint64_t) Message::Priority::Lowest + 1), false);
    }
BOOST_AUTO_TEST_SUITE_END()