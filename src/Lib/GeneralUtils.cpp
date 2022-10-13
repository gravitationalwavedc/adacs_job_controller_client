//
// Created by lewis on 9/4/22.
//

#include "../Settings.h"
#include "Exceptions/my_exception_tracer_lib.h"
#include "GeneralUtils.h"
#include <boost/asio/deadline_timer.hpp>
#include <boost/asio/io_service.hpp>
#include <boost/asio/ip/tcp.hpp>
#include <boost/lexical_cast.hpp>
#include <boost/uuid/uuid_generators.hpp>
#include <boost/uuid/uuid_io.hpp>
#include <climits>
#include <folly/experimental/exception_tracer/ExceptionTracer.h>
#include <fstream>
#include <iostream>
#include <thread>

#ifdef BUILD_TESTS
bool applicationAborted = false;
#endif

void abortApplication() {
#ifdef BUILD_TESTS
    applicationAborted = true;
    std::cerr << "APPLICATION ABORTING" << std::endl;
    throw std::runtime_error("Aborted");
#else
    google::FlushLogFilesUnsafe(google::INFO);
    fflush(stdout);
    fflush(stderr);
    std::abort();
#endif
}

auto getBundlePath() -> std::string {
    return (getExecutablePath() / "bundles" / "unpacked").string();
}

auto getExecutablePath() -> boost::filesystem::path {
    std::array<char, PATH_MAX> result = {0};
    ssize_t count = readlink("/proc/self/exe", result.data(), PATH_MAX);
    return boost::filesystem::path{std::string(result.data(), (count > 0) ? count : 0)}.parent_path();
}

auto readClientConfig() -> nlohmann::json {
    static nlohmann::json configData;

    if (configData.empty()) {
        std::ifstream file((getExecutablePath() / CLIENT_CONFIG_FILE).string());
        configData = nlohmann::json::parse(file);
    }

    return configData;
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

void dumpExceptions(const std::exception& exception) {
    folly::exception_tracer::getCxaRethrowCallbacks().invoke();
    LOG(INFO) << "--- Exception: " << exception.what();
    auto exceptions = folly::exception_tracer::getCurrentExceptions();
    for (auto& exc : exceptions) {
        LOG(INFO) << exc;
    }

    google::FlushLogFilesUnsafe(google::INFO);
    fflush(stdout);
    fflush(stderr);
}

auto getDefaultJobDetails() -> nlohmann::json {
    /*
    Returns the default 'details' dictionary that is passed to the bundle.py file in each bundle

    :return: The default details dictionary
    */

    return {
            {"cluster", readClientConfig()["cluster"]}
    };
}

auto splitString(const std::string& str, const std::string& delimiter) -> std::vector<std::string>
{
    std::vector<std::string> strings;

    std::string::size_type pos; // NOLINT(cppcoreguidelines-init-variables)
    std::string::size_type prev = 0;
    while ((pos = str.find(delimiter, prev)) != std::string::npos)
    {
        strings.push_back(str.substr(prev, pos - prev));
        prev = pos + delimiter.size();
    }

    // To get the last substring (or only, if delimiter is not found)
    strings.push_back(str.substr(prev));

    return strings;
}