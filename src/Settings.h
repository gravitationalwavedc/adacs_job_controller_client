//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_SETTINGS_H
#define ADACS_JOB_CLIENT_SETTINGS_H

#ifdef BUILD_TESTS
#define CLIENT_CONFIG_FILE "test_config.json"
#else
#define CLIENT_CONFIG_FILE "config.json"
#endif

#endif //ADACS_JOB_CLIENT_SETTINGS_H
