//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_SETTINGS_H
#define ADACS_JOB_CLIENT_SETTINGS_H

#include <cstdint>

const uint64_t MESSAGE_INITIAL_VECTOR_SIZE = (1024ULL*64ULL);

#ifdef BUILD_TESTS
#define STRINGIFY2(X) #X
#define STRINGIFY(X) STRINGIFY2(X)
#define CLIENT_CONFIG_FILE "test_config.json"
#define TEST_SERVER_HOST "127.0.0.1"
#define TEST_SERVER_PORT 8001
#define TEST_SERVER_URL TEST_SERVER_HOST ":" STRINGIFY(TEST_SERVER_PORT) "/ws/"
#else
#define CLIENT_CONFIG_FILE "config.json"
#endif

#endif //ADACS_JOB_CLIENT_SETTINGS_H
