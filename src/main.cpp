#include "Bundle/BundleManager.h"
#include "Jobs/JobHandling.h"
#include "Settings.h"
#include "Websocket/WebsocketInterface.h"
#include <boost/filesystem.hpp>
#include <iostream>


[[noreturn]] auto run(const std::string& wsToken) -> int {
    // Start and connect the websocket
    WebsocketInterface::SingletonFactory(wsToken);
    auto websocketInterface = WebsocketInterface::Singleton();
    websocketInterface->start();

    // Wait until the server has notified us that it's ready for packets
    while (!WebsocketInterface::Singleton()->isServerReady()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }

    // Check job status forever
    while (true) {
        checkAllJobsStatus();
        std::this_thread::sleep_for(std::chrono::seconds(JOB_CHECK_SECONDS));
    }
}


auto main(int argc, char* argv[]) -> int {
    if (argc != 2) {
        std::cerr << "Please provide the websocket token to connect with." << std::endl;
        return 1;
    }

    // Set logging defaults
    FLAGS_log_dir = (getExecutablePath() / "logs").string();
    boost::filesystem::create_directories(FLAGS_log_dir);

    FLAGS_minloglevel = GLOG_MIN_LOG_LEVEL;

    // NOLINTBEGIN(cppcoreguidelines-pro-bounds-pointer-arithmetic)
    google::InitGoogleLogging(argv[0]);

    auto wsToken = std::string(argv[1]);
    // NOLINTEND(cppcoreguidelines-pro-bounds-pointer-arithmetic)

    /*
        do the UNIX double-fork magic, see Stevens' "Advanced
        Programming in the UNIX Environment" for details (ISBN 0201563177)
        http://www.erlenstar.demon.co.uk/unix/faq_2.html#SEC16
    */

    auto pid = fork();
    if (pid > 0) {
        // exit first parent
        return 0;
    }

    if (pid == -1) {
        LOG(ERROR) << "fork #1 failed";
        return 1;
    }

    // decouple from parent environment
    chdir("/");
    setsid();
    umask(0);

    // do second fork
    pid = fork();
    if (pid > 0) {
        // exit from second parent
        return 0;
    }

    if (pid == -1) {
        LOG(ERROR) << "fork #2 failed";
        return 1;
    }

    // redirect standard file descriptors
    std::cout << std::flush;
    std::cerr << std::flush;

    // NOLINTBEGIN(cppcoreguidelines-pro-type-vararg,clang-analyzer-core.NonNullParamChecker,cppcoreguidelines-pro-type-vararg,hicpp-vararg,google-readability-casting,cppcoreguidelines-pro-type-cstyle-cast)
    auto sIn = open((const char*) STDIN_FILENO, O_RDONLY);
    auto sOut = open((const char*) STDOUT_FILENO, O_APPEND | O_WRONLY);
    auto sErr = open((const char*) STDERR_FILENO, O_APPEND | O_WRONLY);
    // NOLINTEND(cppcoreguidelines-pro-type-vararg,clang-analyzer-core.NonNullParamChecker,cppcoreguidelines-pro-type-vararg,hicpp-vararg,google-readability-casting,cppcoreguidelines-pro-type-cstyle-cast)

    dup2(sIn, STDIN_FILENO);
    dup2(sOut, STDOUT_FILENO);
    dup2(sErr, STDERR_FILENO);

    run(wsToken);
}