//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_JSONCONFIGFIXTURE_H
#define ADACS_JOB_CLIENT_JSONCONFIGFIXTURE_H

#include <fstream>
#include "nlohmann/json.hpp"
#include "../../Settings.h"
#include "../../lib/GeneralUtils.h"

class JsonConfigFixture {
public:
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    nlohmann::json clientConfig = {
            {"websocketEndpoint", TEST_SERVER_URL}
    };
    std::string clientConfigFile = (getExecutablePath() / CLIENT_CONFIG_FILE).string();
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JsonConfigFixture() {
        writeClientConfig();
    }

    ~JsonConfigFixture() {
        std::filesystem::remove(clientConfigFile);
    }

    void writeClientConfig() {
        std::ofstream file(clientConfigFile, std::ios_base::trunc);
        file << clientConfig.dump();
    }
};

#endif //ADACS_JOB_CLIENT_JSONCONFIGFIXTURE_H
