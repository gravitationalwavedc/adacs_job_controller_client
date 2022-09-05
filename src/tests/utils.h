//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_UTILS_H
#define ADACS_JOB_CLIENT_UTILS_H

#include "server_wss.hpp"

using TestWsServer = SimpleWeb::SocketServer<SimpleWeb::WS>;

auto randomInt(uint64_t start, uint64_t end) -> uint64_t;
auto generateRandomData(uint32_t count) -> std::shared_ptr<std::vector<uint8_t>>;

#endif //ADACS_JOB_CLIENT_UTILS_H
