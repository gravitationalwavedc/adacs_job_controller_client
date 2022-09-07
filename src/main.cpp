#include <iostream>
#include "Bundle/BundleManager.h"
#include "Websocket/WebsocketInterface.h"

int main(int argc, char* argv[]) {
    if (argc != 2) {
        std::cerr << "Please provide the websocket token to connect with." << std::endl;
        return 1;
    }

    auto wsToken = std::string(argv[1]);

    auto bundleManager = std::make_shared<BundleManager>();

    // Start and connect the websocket
    WebsocketInterface::SingletonFactory(wsToken);
    auto websocketInterface = WebsocketInterface::Singleton();
    websocketInterface->start();

//    auto pythonInterface = PythonInterface("libpython3.so");
//    auto bundle1 = new BundleInterface("/home/lewis/Projects/gwdc/adacs_job_client/src/testing/bundle");
//    std::thread t1{[bundle1] { bundle1->f("t1"); }};
//    std::thread t2{[bundle1] { bundle1->f("t2"); }};
//    std::thread t3{[bundle1] { bundle1->f("t3"); }};
//    std::thread t4{[bundle1] { bundle1->f("t4"); }};
//    std::thread t5{[bundle1] { bundle1->f("t5"); }};
//
//    auto bundle2 = new BundleInterface("/home/lewis/Projects/gwdc/adacs_job_client/src/testing/bundle");
//    std::thread t6{[bundle2] { bundle2->f("t1"); }};
//    std::thread t7{[bundle2] { bundle2->f("t2"); }};
//    std::thread t8{[bundle2] { bundle2->f("t3"); }};
//    std::thread t9{[bundle2] { bundle2->f("t4"); }};
//    std::thread t10{[bundle2] { bundle2->f("t5"); }};
//
//    auto threadScope = PythonInterface::enableThreading();
//
//    t1.join();
//    t2.join();
//    t3.join();
//    t4.join();
//    t5.join();
//    t6.join();
//    t7.join();
//    t8.join();
//    t9.join();
//    t10.join();
//
//    std::make_shared<WsClient>("test");

    std::cout << "Hello, World!" << std::endl;
    return 0;
}
