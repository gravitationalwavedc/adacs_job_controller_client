//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_SETTINGS_H
#define ADACS_JOB_CLIENT_SETTINGS_H

#include <cstdint>

const uint32_t GLOG_MIN_LOG_LEVEL = 0;

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

#define PING_INTERVAL_SECONDS 10
#define QUEUE_SOURCE_PRUNE_SECONDS 60

#define DATABASE_FILE "db.sqlite3"

#define CHUNK_SIZE (1024ULL*64ULL)
#define CHUNK_WAIT_COUNT 10

#define JOB_CHECK_SECONDS 60

// Wait up to 60 minutes for the job to submit, before giving up and retrying. This can happen if the bundle is under
// a lot of load and it takes a long time to respond.
#define MAX_SUBMIT_COUNT 60

#define GITHUB_ENDPOINT "api.github.com"
#define GITHUB_LATEST_URL "/repos/gravitationalwavedc/adacs_job_controller_client/releases/latest"

#endif //ADACS_JOB_CLIENT_SETTINGS_H
