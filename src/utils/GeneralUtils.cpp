//
// Created by lewis on 9/4/22.
//

#include <climits>
#include <boost/filesystem/path.hpp>
#include <fstream>
#include "GeneralUtils.h"
#include "../Settings.h"

auto readClientConfig() -> nlohmann::json {
    char result[PATH_MAX] = {0};
    ssize_t count = readlink("/proc/self/exe", result, PATH_MAX);
    auto execPath = boost::filesystem::path(std::string(result, (count > 0) ? count : 0));

    std::ifstream file((execPath.parent_path() / CLIENT_CONFIG_FILE).string());
    return nlohmann::json::parse(file);
}
