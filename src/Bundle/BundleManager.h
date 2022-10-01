//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_BUNDLEMANAGER_H
#define ADACS_JOB_CLIENT_BUNDLEMANAGER_H

#include <string>
#include <memory>
#include "PythonInterface.h"
#include "nlohmann/json.hpp"
#include "BundleInterface.h"

class BundleManager {
public:
    BundleManager();

    static auto Singleton() -> std::shared_ptr<BundleManager>;

    auto runBundle_string(std::string bundleFunction, std::string bundleHash, nlohmann::json details, std::string jobData) -> std::string;
    auto runBundle_uint64(std::string bundleFunction, std::string bundleHash, nlohmann::json details, std::string jobData) -> uint64_t;

private:
    std::shared_ptr<BundleInterface> loadBundle(std::string bundleHash);

    std::shared_ptr<PythonInterface> pythonInterface;
    std::map<std::string, std::shared_ptr<BundleInterface>> bundles;
};


#endif //ADACS_JOB_CLIENT_BUNDLEMANAGER_H
