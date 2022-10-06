#include <iostream>
#include "Bundle/BundleManager.h"
#include "Websocket/WebsocketInterface.h"
#include "Settings.h"


int run(std::string wsToken) {
    auto bundleManager = std::make_shared<BundleManager>();

    // Start and connect the websocket
//    WebsocketInterface::SingletonFactory(wsToken);
//    auto websocketInterface = WebsocketInterface::Singleton();
//    websocketInterface->start();

    BundleInterface* bundle1, *bundle2;
    std::thread b1{[&bundle1] {
        bundle1 = new BundleInterface("a");
    }};
    std::thread b2{[&bundle2] {
        bundle2 = new BundleInterface("b");
    }};
    b1.join();
    b2.join();

//
//    bundle1 = new BundleInterface("a");
//    bundle2 = new BundleInterface("b");

    auto threadScope = PythonInterface::enableThreading();

    std::thread t1{[bundle1] {
        bundle1->f("t1");
    }};
    std::thread t2{[bundle1] { bundle1->f("t2"); }};
    std::thread t3{[bundle1] { bundle1->f("t3"); }};
    std::thread t4{[bundle1] { bundle1->f("t4"); }};
    std::thread t5{[bundle1] { bundle1->f("t5"); }};

    std::thread t6{[bundle2] { bundle2->f("t1"); }};
    std::thread t7{[bundle2] { bundle2->f("t2"); }};
    std::thread t8{[bundle2] { bundle2->f("t3"); }};
    std::thread t9{[bundle2] { bundle2->f("t4"); }};
    std::thread t10{[bundle2] { bundle2->f("t5"); }};

    t1.join();
    t2.join();
    t3.join();
    t4.join();
    t5.join();
    t6.join();
    t7.join();
    t8.join();
    t9.join();
    t10.join();

//    std::make_shared<WsClient>("test");

    std::cout << "Hello, World!" << std::endl;
    return 0;
}


int main(int argc, char* argv[]) {
    if (argc != 2) {
        std::cerr << "Please provide the websocket token to connect with." << std::endl;
        return 1;
    }

    // Set logging defaults
    FLAGS_log_dir = (getExecutablePath() / "logs").string();
    FLAGS_minloglevel = GLOG_MIN_LOG_LEVEL;
    google::InitGoogleLogging(argv[0]);

    auto wsToken = std::string(argv[1]);

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
        std::cerr << "fork #1 failed" << std::endl;
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
        std::cerr << "fork #2 failed" << std::endl;
        return 1;
    }

    // redirect standard file descriptors
    std::cout << std::flush;
    std::cerr << std::flush;

    auto si = open((const char*) STDIN_FILENO, O_RDONLY);
    auto so = open((const char*) STDOUT_FILENO, O_APPEND | O_WRONLY);
    auto se = open((const char*) STDERR_FILENO, O_APPEND | O_WRONLY);

    dup2(si, STDIN_FILENO);
    dup2(so, STDOUT_FILENO);
    dup2(se, STDERR_FILENO);

    return run(wsToken);
}