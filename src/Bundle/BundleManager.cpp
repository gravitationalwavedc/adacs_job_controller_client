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
    if (!singleton) {
        singleton = std::make_shared<BundleManager>();
    }

    return singleton;
}

auto BundleManager::runBundle_string(std::string bundleFunction, std::string bundleHash, nlohmann::json details, std::string jobData) -> std::string {
    auto bundle = loadBundle(bundleHash);

    auto resultObject = bundle->run(bundleFunction, details, jobData);
    auto result = bundle->toString(resultObject);
    bundle->disposeObject(resultObject);

    return result;
}

auto BundleManager::runBundle_uint64(std::string bundleFunction, std::string bundleHash, nlohmann::json details, std::string jobData) -> uint64_t {
    auto bundle = loadBundle(bundleHash);

    auto resultObject = bundle->run(bundleFunction, details, jobData);
    auto result = bundle->toUint64(resultObject);
    bundle->disposeObject(resultObject);

    return result;
}

std::shared_ptr<BundleInterface> BundleManager::loadBundle(std::string bundleHash) {
    if (bundles.contains(bundleHash)) {
        // Bundle is already loaded
        return bundles[bundleHash];
    }

    // Load the bundle
    bundles.emplace(bundleHash, std::make_shared<BundleInterface>(bundleHash));

    return bundles[bundleHash];
}
