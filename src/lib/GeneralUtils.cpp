//
// Created by lewis on 9/4/22.
//

#include "../Settings.h"
#include "GeneralUtils.h"
#include <boost/asio/deadline_timer.hpp>
#include <boost/asio/io_service.hpp>
#include <boost/asio/ip/tcp.hpp>
#include <boost/lexical_cast.hpp>
#include <climits>
#include <fstream>
#include <thread>
#include <boost/uuid/uuid_generators.hpp>
#include <boost/uuid/uuid_io.hpp>

auto getExecutablePath() -> boost::filesystem::path {
    char result[PATH_MAX] = {0};
    ssize_t count = readlink("/proc/self/exe", result, PATH_MAX);
    return boost::filesystem::path{std::string(result, (count > 0) ? count : 0)}.parent_path();
}

auto readClientConfig() -> nlohmann::json {
    std::ifstream file((getExecutablePath() / CLIENT_CONFIG_FILE).string());
    return nlohmann::json::parse(file);
}

// NOLINTBEGIN(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)
auto acceptingConnections(uint16_t port) -> bool {
    using boost::asio::io_service, boost::asio::deadline_timer, boost::asio::ip::tcp;
    using ec = boost::system::error_code;

    bool result = false;

    for (auto counter = 0; counter < 10 && !result; counter++) {
        try {
            io_service svc;
            tcp::socket socket(svc);
            deadline_timer tim(svc, boost::posix_time::milliseconds(100));

            tim.async_wait([&](ec) { socket.cancel(); });
            socket.async_connect({{}, port}, [&](ec errorCode) {
                result = !errorCode;
            });

            svc.run();
        } catch(...) { }

        if (!result) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }
    }

    return result;
}
// NOLINTEND(cppcoreguidelines-avoid-magic-numbers,readability-magic-numbers)

auto generateUUID() -> std::string {
    return boost::lexical_cast<std::string>(boost::uuids::random_generator()());
}