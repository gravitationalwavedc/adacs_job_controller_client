cmake_minimum_required(VERSION 3.13)
project(adacs_job_client)

set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_CXX_STANDARD 20)

# Generate maximum debug information (Dwarf version 4 for folly backtrace support)
set(CMAKE_CXX_FLAGS_DEBUG " -O0 -fpermissive -static-libstdc++ -static-libgcc -w -g -rdynamic -gdwarf-4  ")
set(CMAKE_C_FLAGS_DEBUG " -O0 -static-libstdc++ -static-libgcc -w -g -rdynamic -gdwarf-4  ")
set(CMAKE_CXX_FLAGS_RELEASE " -O2 -fpermissive -static-libstdc++ -static-libgcc -w -g -rdynamic -gdwarf-4  ")
set(CMAKE_C_FLAGS_RELEASE " -O2 -static-libstdc++ -static-libgcc -w -g -rdynamic -gdwarf-4  ")
if (CMAKE_BUILD_TYPE STREQUAL "Release")
    message("Enabling LTO for release build")
    set(CMAKE_INTERPROCEDURAL_OPTIMIZATION TRUE)
ENDIF(CMAKE_BUILD_TYPE STREQUAL "Release")

# Suppress nested inexact package name searches
set(FPHSA_NAME_MISMATCHED 1)

# We want to control variables in the child projects from here
set(CMAKE_POLICY_DEFAULT_CMP0077 NEW)

# Force LTO
set(CMAKE_POLICY_DEFAULT_CMP0069 NEW)

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
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(LIBFMT_LIBRARY ${CMAKE_BINARY_DIR}/third_party/fmt/libfmtd.a)
else()
    set(LIBFMT_LIBRARY ${CMAKE_BINARY_DIR}/third_party/fmt/libfmt.a)
endif(CMAKE_BUILD_TYPE STREQUAL "Debug")

set(WITH_GFLAGS OFF)
set(WITH_GTEST OFF)
set(WITH_PKGCONFIG OFF)
set(WITH_UNWIND OFF)
set(Glog_ROOT ${CMAKE_SOURCE_DIR}/third_party/glog)
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(Glog_LIBRARY ${CMAKE_BINARY_DIR}/third_party/glog/libglogd.a)
else()
    set(Glog_LIBRARY ${CMAKE_BINARY_DIR}/third_party/glog/libglog.a)
endif(CMAKE_BUILD_TYPE STREQUAL "Debug")

set(LIBDWARF_LIBRARIES ${CMAKE_SOURCE_DIR}/third_party/elfutils_install/lib/libdw.a)
set(LIBDWARF_INCLUDE_DIRS ${CMAKE_SOURCE_DIR}/third_party/elfutils_install/include/)
set(LIBDWARF_FOUND TRUE)

# Set up boost
set(Boost_USE_STATIC_LIBS ON)
set(Boost_USE_MULTITHREAD ON)
find_package(Boost 1.67 REQUIRED COMPONENTS unit_test_framework filesystem url)
link_libraries(${Boost_UNIT_TEST_FRAMEWORK_LIBRARY})

include_directories(
        third_party/boost_install/include
        third_party/json/include
        third_party/date/include
        third_party/sqlpp11/include
        third_party/Simple-WebSocket-Server
        third_party/Simple-Web-Server
        third_party/folly
        third_party/fmt/include
        third_party/libevent/include
        third_party/sqlite3-cmake/src
        third_party/mempatch
        third_party/elfutils_install/include
        third_party/cpp-subprocess
        third_party/semver/include
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
file(WRITE third_party/folly/folly/experimental/exception_tracer/ExceptionTracerLib.cpp "")
set(LIBGFLAGS_FOUND FALSE)

add_subdirectory(third_party/folly)
add_subdirectory(third_party/fmt)
add_subdirectory(third_party/openssl-cmake)
add_subdirectory(third_party/Simple-WebSocket-Server)
add_subdirectory(third_party/Simple-Web-Server)
add_subdirectory(third_party/sqlpp11)
add_subdirectory(third_party/sqlite3-cmake)
add_subdirectory(third_party/mempatch)
add_subdirectory(third_party/cpp-subprocess)
add_subdirectory(third_party/semver)

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
        Lib/GeneralUtils.cpp
        Lib/GeneralUtils.h
        Lib/Messaging/Message.cpp
        Lib/Messaging/Message.h
        Core/MessageHandler.cpp
        Core/MessageHandler.h
        Files/FileList.cpp
        Files/FileDownload.cpp
        Files/FileHandling.h
        Lib/jobclient_schema.h
        Lib/Exceptions/my_eh_throw.cpp
        Lib/Exceptions/unwind-cxx.h
        Lib/Exceptions/eh_atomics.h
        Lib/Exceptions/my_exception_tracer_lib.h
        Lib/Exceptions/my_eh_ptr.cpp
        Lib/Exceptions/my_eh_catch.cpp
        Lib/Exceptions/my_exception_tracer_lib.cpp
        Lib/Exceptions/segvcatch.h
        Lib/Exceptions/segvcatch.cpp
        Lib/Exceptions/x86_64-signal.h
        Jobs/Submit.cpp
        Jobs/JobHandling.h
        Jobs/CheckStatus.cpp
        Lib/JobStatus.h
        Jobs/ArchiveJob.cpp
        DB/sJob.h
        DB/sStatus.h
        Bundle/BundleDB.cpp
        Bundle/BundleDB.h
        Lib/UpdateCheck.cpp
        Version.h
        Jobs/CancelJob.cpp
        Jobs/DeleteJob.cpp
        Bundle/BundleLogging.cpp
        Bundle/BundleLogging.h
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
        Tests/test_main.cpp
        Tests/utils.h
        Tests/fixtures/WebsocketServerFixture.h
        Websocket/tests/WebsocketInterfaceTests.cpp
        Tests/fixtures/JsonConfigFixture.h
        Lib/Messaging/tests.cpp
        Tests/utils.cpp
        Websocket/tests/ping_pong_tests.cpp
        Files/tests/test_file_list.cpp
        Tests/fixtures/TemporaryDirectoryFixture.h
        Tests/fixtures/BundleFixture.h
        Files/tests/test_file_download.cpp
        Tests/fixtures/BundleFixture.cpp
        Jobs/tests/test_job_submit.cpp
        Tests/fixtures/DatabaseFixture.h
        Jobs/tests/test_check_status.cpp
        Jobs/tests/test_archive_job.cpp
        Jobs/tests/test_check_status_many_bundles.cpp
        Tests/db/SqliteConnector.h
        Tests/db/sJobDb.h
        Tests/db/sStatusDb.h
        Bundle/tests/bundle_db_tests.cpp
        Jobs/tests/test_cancel_job.cpp
        Jobs/tests/test_delete_job.cpp
        Bundle/tests/bundle_logging_tests.cpp
)

# Set the link libraries
set(
        LINK_LIBRARIES
        dl
        ssl
        crypto
        folly
        double-conversion
        sqlite3
        mempatch
        event
        ${CMAKE_SOURCE_DIR}/third_party/elfutils_install/lib/libdw.a
        ${CMAKE_SOURCE_DIR}/third_party/elfutils_install/lib/libelf.a
        ${CMAKE_SOURCE_DIR}/third_party/boost_install/lib/libboost_filesystem.a
        ${CMAKE_SOURCE_DIR}/third_party/boost_install/lib/libboost_unit_test_framework.a
        ${CMAKE_SOURCE_DIR}/third_party/boost_install/lib/libboost_url.a
        ${Glog_LIBRARY}
)

set_target_properties(
        Boost_Tests_run
        PROPERTIES
        CXX_CLANG_TIDY "clang-tidy"
)

target_link_libraries(adacs_job_client ${LINK_LIBRARIES})
target_link_libraries(Boost_Tests_run ${LINK_LIBRARIES} gcov)

# Configure custom definitions for testing
target_compile_definitions(Boost_Tests_run PUBLIC -DBUILD_TESTS)
target_compile_options(Boost_Tests_run PUBLIC -fprofile-arcs -ftest-coverage -fPIC -Wall)

# Hacks
set_target_properties(folly_deps PROPERTIES INTERFACE_INCLUDE_DIRECTORIES "")

file(COPY third_party/glog/src/glog/platform.h DESTINATION ${CMAKE_BINARY_DIR}/third_party/glog/glog/)
file(COPY third_party/glog/src/glog/log_severity.h DESTINATION ${CMAKE_BINARY_DIR}/third_party/glog/glog/)
