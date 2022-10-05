//
// Created by lewis on 9/4/22.
//

#include <iostream>
#include "BundleManager.h"
#include "../lib/GeneralUtils.h"

static std::shared_ptr<BundleManager> singleton;

BundleManager::BundleManager() {
    auto config = readClientConfig();

    pythonInterface = std::make_shared<PythonInterface>();
    pythonInterface->initPython(config["pythonLibrary"]);
}

auto BundleManager::Singleton() -> std::shared_ptr<BundleManager> {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> lock(mutex_);

    if (!singleton) {
        singleton = std::make_shared<BundleManager>();
    }

    return singleton;
}

auto BundleManager::runBundle_string(const std::string& bundleFunction, const std::string& bundleHash, const nlohmann::json& details, const std::string& jobData) -> std::string {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> lock(mutex_);

    auto bundle = loadBundle(bundleHash);

    auto resultObject = bundle->run(bundleFunction, details, jobData);
    auto result = bundle->toString(resultObject);
    bundle->disposeObject(resultObject);

    return result;
}

auto BundleManager::runBundle_uint64(const std::string& bundleFunction, const std::string& bundleHash, const nlohmann::json& details, const std::string& jobData) -> uint64_t {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> lock(mutex_);

    auto bundle = loadBundle(bundleHash);

    auto *resultObject = bundle->run(bundleFunction, details, jobData);
    auto result = bundle->toUint64(resultObject);
    bundle->disposeObject(resultObject);

    return result;
}

auto BundleManager::runBundle_json(const std::string& bundleFunction, const std::string& bundleHash, const nlohmann::json& details,
                                   const std::string& jobData) -> nlohmann::json {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> lock(mutex_);

    auto bundle = loadBundle(bundleHash);

    auto *resultObject = bundle->run(bundleFunction, details, jobData);
    auto result = bundle->jsonDumps(resultObject);
    bundle->disposeObject(resultObject);

    return nlohmann::json::parse(result);
}

auto BundleManager::loadBundle(const std::string& bundleHash) -> std::shared_ptr<BundleInterface> {
    static std::shared_mutex mutex_;
    std::unique_lock<std::shared_mutex> lock(mutex_);

    if (bundles.contains(bundleHash)) {
        // Bundle is already loaded
        return bundles[bundleHash];
    }

    // Load the bundle
    bundles.emplace(bundleHash, std::make_shared<BundleInterface>(bundleHash));

    return bundles[bundleHash];
}
