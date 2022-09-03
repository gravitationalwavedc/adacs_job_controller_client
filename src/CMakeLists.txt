cmake_minimum_required(VERSION 3.13)
project(adacs_job_client)

set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_CXX_STANDARD 11)

# Generate maximum debug information (Dwarf version 4 for folly backtrace support)
set(CMAKE_CXX_FLAGS_DEBUG " -g -O0 -rdynamic -gdwarf-4 -fpermissive -w -static-libstdc++ -static-libgcc ")

# Suppress nested inexact package name searches
set(FPHSA_NAME_MISMATCHED 1)

# We want to control variables in the child projects from here
set(CMAKE_POLICY_DEFAULT_CMP0077 NEW)

find_package (Python3 COMPONENTS Development)

# Set up the hinnant date library
set(HinnantDate_ROOT_DIR Lib/date)
set(HinnantDate_INCLUDE_DIR Lib/date/include)

# Set config for nested libraries
set(ENABLE_TESTS Off)
set(ENABLE_DATE_TESTING Off)
set(CMAKE_LIBRARY_ARCHITECTURE "x86_64-linux-gnu")
set(Boost_NO_WARN_NEW_VERSIONS On)
set(BOOST_ROOT ${CMAKE_SOURCE_DIR}/third_party/boost_install)
set(OPENSSL_ROOT_DIR ${CMAKE_SOURCE_DIR}/third_party/openssl-cmake)
set(OPENSSL_INCLUDE_DIR ${CMAKE_SOURCE_DIR}/third_party/openssl-cmake/include)
set(OPENSSL_CRYPTO_LIBRARY "crypto")

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
        ${Python3_INCLUDE_DIRS}
)

# Include the nested libraries
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
        utils/GeneralUtils.cpp
        utils/GeneralUtils.h
)

# Set the main executable and source files
add_executable(
        adacs_job_client
        ${SOURCE_FILES}
        main.cpp)

# Set the test executable and source files
add_executable(
        Boost_Tests_run
        ${SOURCE_FILES}
        tests/test_main.cpp
)

# Set the link libraries
set(
        LINK_LIBRARIES
        dl
        ssl
        crypto
        ${Boost_UNIT_TEST_FRAMEWORK_LIBRARY}
        ${Boost_FILESYSTEM_LIBRARY}
)

set_target_properties(
        adacs_job_client
        PROPERTIES
        CXX_CLANG_TIDY "clang-tidy"
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