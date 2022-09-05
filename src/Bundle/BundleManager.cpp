//
// Created by lewis on 9/4/22.
//

#include "BundleManager.h"
#include "../lib/GeneralUtils.h"


BundleManager::BundleManager() {
    auto config = readClientConfig();
}
