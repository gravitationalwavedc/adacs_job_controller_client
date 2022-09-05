//
// Created by lewis on 9/5/22.
//

#include <boost/test/unit_test.hpp>
#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../WebsocketInterface.h"

struct WebsocketInterfaceFixture : public WebsocketServerFixture {

};

BOOST_FIXTURE_TEST_SUITE(websocket_interface_tests, WebsocketInterfaceFixture)
    BOOST_AUTO_TEST_CASE(test_constructor) {
        std::string token = generateUUID();
        auto websocketInterface = std::make_unique<WebsocketInterface>(token);

        BOOST_CHECK_EQUAL(*websocketInterface->geturl(), std::string{TEST_SERVER_URL} + "?token=" + token);
    }
}