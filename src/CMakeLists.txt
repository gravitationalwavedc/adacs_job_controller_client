cmake_minimum_required(VERSION 3.13)
project(adacs_job_client)

set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_CXX_STANDARD 20)

# Generate maximum debug information (Dwarf version 4 for folly backtrace support)
set(CMAKE_CXX_FLAGS_DEBUG " -g -O0 -rdynamic -gdwarf-4 -fpermissive -w -static-libstdc++ -static-libgcc ")
set(CMAKE_C_FLAGS_DEBUG " -g -O0 -rdynamic -gdwarf-4 -fpermissive -w -static-libstdc++ -static-libgcc ")

# Suppress nested inexact package name searches
set(FPHSA_NAME_MISMATCHED 1)

# We want to control variables in the child projects from here
set(CMAKE_POLICY_DEFAULT_CMP0077 NEW)

# Configure extension policy
set(CMAKE_POLICY_DEFAULT_CMP0115 OLD)

# Configure _ROOT variables
set(CMAKE_POLICY_DEFAULT_CMP0074 NEW)

# Add our own cmake path so that we can override the find SSL
set(CMAKE_MODULE_PATH ${CMAKE_CURRENT_SOURCE_DIR}/cmake ${CMAKE_MODULE_PATH})

# Disable system packages
set(CMAKE_DISABLE_FIND_PACKAGE_LibLZMA TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_ZLIB TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_Gflags TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_BZip2 TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_LZ4 TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_Zstd TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_Snappy TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_LibDwarf TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_Libiberty TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_LibUring TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_Libsodium TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_LibUnwind TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_LibAIO TRUE)
set(CMAKE_DISABLE_FIND_PACKAGE_fmt TRUE)

find_package (Python3 COMPONENTS Development)

# Set up the hinnant date library
set(HinnantDate_ROOT_DIR Lib/date)
set(HinnantDate_INCLUDE_DIR Lib/date/include)

# Set config for nested libraries
set(ENABLE_TESTS Off)
set(ENABLE_DATE_TESTING Off)
set(CMAKE_LIBRARY_ARCHITECTURE "x86_64-linux-gnu")
set(FOLLY_NO_EXCEPTION_TRACER ON)

set(Boost_NO_WARN_NEW_VERSIONS ON)
set(BOOST_ROOT ${CMAKE_SOURCE_DIR}/third_party/boost_install)

set(OPENSSL_ROOT_DIR ${CMAKE_SOURCE_DIR}/third_party/openssl-cmake)
set(OPENSSL_INCLUDE_DIR ${CMAKE_SOURCE_DIR}/third_party/openssl-cmake/include)
set(OPENSSL_CRYPTO_LIBRARY "crypto")
set(OPENSSL_SSL_LIBRARY "ssl")
set(OPENSSL_LIBRARIES dl)

set(DOUBLE_CONVERSION_ROOT ${CMAKE_SOURCE_DIR}/third_party/double-conversion)
set(DOUBLE_CONVERSION_INCLUDE_DIR ${CMAKE_SOURCE_DIR}/third_party/double-conversion)
set(DOUBLE_CONVERSION_LIBRARY "double-conversion")

set(FMT_ROOT ${CMAKE_SOURCE_DIR}/third_party/fmt)
set(LIBFMT_INCLUDE_DIR ${CMAKE_SOURCE_DIR}/third_party/fmt/include)
set(LIBFMT_LIBRARY ${CMAKE_BINARY_DIR}/third_party/fmt/libfmtd.a)

set(WITH_GFLAGS OFF)
set(WITH_GTEST OFF)
set(WITH_PKGCONFIG OFF)
set(WITH_UNWIND OFF)
set(Glog_ROOT ${CMAKE_SOURCE_DIR}/third_party/glog)
set(Glog_LIBRARY ${CMAKE_BINARY_DIR}/third_party/glog/libglogd.a)

# Set up boost
set(Boost_USE_STATIC_LIBS ON)
set(Boost_USE_MULTITHREAD ON)
find_package(Boost 1.67 REQUIRED COMPONENTS unit_test_framework filesystem)
link_libraries(${Boost_UNIT_TEST_FRAMEWORK_LIBRARY})

include_directories(
        third_party/boost_install/include
        third_party/json/include
        third_party/date/include
        third_party/sqlpp11/include
        third_party/Simple-WebSocket-Server
        third_party/folly
        third_party/fmt/include
        third_party/libevent/include
        ${CMAKE_BINARY_DIR}/third_party/folly
        ${CMAKE_BINARY_DIR}/third_party/openssl-cmake/include
        ${CMAKE_BINARY_DIR}/third_party/glog
        ${CMAKE_BINARY_DIR}/third_party/libevent/include
        ${Python3_INCLUDE_DIRS}
)

# Include the nested libraries
add_subdirectory(third_party/double-conversion)

set(EVENT__DISABLE_BENCHMARK TRUE)
set(EVENT__DISABLE_TESTS TRUE)
add_subdirectory(third_party/libevent)

add_subdirectory(third_party/glog)

file(REMOVE third_party/folly/build/fbcode_builder/CMake/FindGlog.cmake)
file(REMOVE third_party/folly/build/fbcode_builder/CMake/FindGflags.cmake)
set(LIBGFLAGS_FOUND FALSE)
add_subdirectory(third_party/folly)

add_subdirectory(third_party/fmt)

add_subdirectory(third_party/openssl-cmake)

add_subdirectory(third_party/Simple-WebSocket-Server)

add_subdirectory(third_party/sqlpp11)

# Define the source files
set(
        SOURCE_FILES
        Bundle/BundleInterface.cpp
        Bundle/BundleInterface.h
        Bundle/PythonInterface.cpp
        Bundle/PythonInterface.h
        Bundle/BundleManager.cpp
        Bundle/BundleManager.h
        Websocket/WebsocketInterface.cpp
        Websocket/WebsocketInterface.h
        lib/GeneralUtils.cpp
        lib/GeneralUtils.h
        lib/Messaging/Message.cpp
        lib/Messaging/Message.h
)

# Set the main executable and source files
add_executable(
        adacs_job_client
        ${SOURCE_FILES}
        main.cpp
)

# Set the test executable and source files
add_executable(
        Boost_Tests_run
        ${SOURCE_FILES}
        tests/test_main.cpp
        tests/utils.h
        tests/fixtures/WebsocketServerFixture.h
        Websocket/tests/WebsocketInterfaceTests.cpp
        tests/fixtures/JsonConfigFixture.h
        lib/Messaging/tests.cpp
        tests/utils.cpp
        Websocket/tests/ping_pong_tests.cpp
)

# Set the link libraries
set(
        LINK_LIBRARIES
        dl
        ssl
        crypto
        folly
        event
        double-conversion
        ${CMAKE_SOURCE_DIR}/third_party/boost_install/lib/libboost_filesystem.a
        ${CMAKE_SOURCE_DIR}/third_party/boost_install/lib/libboost_unit_test_framework.a
        ${Glog_LIBRARY}
)

set_target_properties(
        adacs_job_client
        PROPERTIES
#        CXX_CLANG_TIDY "clang-tidy"
        CXX_FLAGS_DEBUG " -g -O0 -rdynamic -gdwarf-4 -fpermissive -Wall -static-libstdc++ -static-libgcc "
)

set_target_properties(
        Boost_Tests_run
        PROPERTIES
#        CXX_CLANG_TIDY "clang-tidy"
        CXX_FLAGS_DEBUG " -g -O0 -rdynamic -gdwarf-4 -fpermissive -Wall -static-libstdc++ -static-libgcc "
)

target_link_libraries(adacs_job_client ${LINK_LIBRARIES})
target_link_libraries(Boost_Tests_run ${LINK_LIBRARIES} gcov)

# Configure custom definitions for testing
target_compile_definitions(Boost_Tests_run PUBLIC -DBUILD_TESTS)
target_compile_options(Boost_Tests_run PUBLIC -fprofile-arcs -ftest-coverage -fPIC)

# Hacks
set_target_properties(folly_deps PROPERTIES INTERFACE_INCLUDE_DIRECTORIES "")

file(COPY third_party/glog/src/glog/platform.h DESTINATION ${CMAKE_BINARY_DIR}/third_party/glog/glog/)
file(COPY third_party/glog/src/glog/log_severity.h DESTINATION ${CMAKE_BINARY_DIR}/third_party/glog/glog/)