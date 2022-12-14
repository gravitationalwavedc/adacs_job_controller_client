//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_BUNDLEMANAGER_H
#define ADACS_JOB_CLIENT_BUNDLEMANAGER_H

#include "BundleInterface.h"
#include "PythonInterface.h"
#include "nlohmann/json.hpp"
#include <memory>
#include <string>

class BundleManager {
public:
    BundleManager();

    static auto Singleton() -> std::shared_ptr<BundleManager>;

    auto runBundle_string(const std::string& bundleFunction, const std::string& bundleHash, const nlohmann::json& details, const std::string& jobData) -> std::string;
    auto runBundle_uint64(const std::string& bundleFunction, const std::string& bundleHash, const nlohmann::json& details, const std::string& jobData) -> uint64_t;
    auto runBundle_json(const std::string& bundleFunction, const std::string& bundleHash, const nlohmann::json& details, const std::string& jobData) -> nlohmann::json;
    auto runBundle_bool(const std::string &bundleFunction, const std::string &bundleHash, const nlohmann::json &details, const std::string &jobData) -> bool;

    auto loadBundle(const std::string& bundleHash) -> std::shared_ptr<BundleInterface>;

private:
    std::shared_ptr<PythonInterface> pythonInterface;
    std::map<std::string, std::shared_ptr<BundleInterface>> bundles;
};


#endif //ADACS_JOB_CLIENT_BUNDLEMANAGER_H
