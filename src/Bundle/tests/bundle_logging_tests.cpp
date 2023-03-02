//
// Created by lewis on 10/9/22.
//


#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/JsonConfigFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"
#include "../BundleManager.h"
#include <boost/test/unit_test.hpp>

struct BundleLoggingTestFixture : public BundleFixture, public JsonConfigFixture {};

std::string lastBundleLoggingMessage;
bool lastBundleLoggingbStdOut;

BOOST_FIXTURE_TEST_SUITE(bundle_logging_test_suite, BundleLoggingTestFixture)
    BOOST_AUTO_TEST_CASE(test_simple_stdout) {
        auto bundleHash = generateUUID();

        auto testMessage = std::string{"'testing stdout'"};

        writeBundleLoggingStdOut(bundleHash, testMessage);

        auto result = BundleManager::Singleton()->runBundle_bool("logging_test", bundleHash, {}, "");

        BOOST_CHECK_EQUAL(result, true);
        BOOST_CHECK_EQUAL(lastBundleLoggingMessage, "Bundle [" + bundleHash + "]: testing stdout");
        BOOST_CHECK_EQUAL(lastBundleLoggingbStdOut, true);
    }

    BOOST_AUTO_TEST_CASE(test_complex_stdout) {
        auto bundleHash = generateUUID();

        auto testMessage = std::string{"'testing stdout', 56, {'a': 'b'}, [45, 'a', sum([5, 4])], (123, 321,), type((1,))"};

        writeBundleLoggingStdOut(bundleHash, testMessage);

        auto result = BundleManager::Singleton()->runBundle_bool("logging_test", bundleHash, {}, "");

        BOOST_CHECK_EQUAL(result, true);
        BOOST_CHECK_EQUAL(lastBundleLoggingMessage, "Bundle [" + bundleHash + "]: testing stdout 56 {'a': 'b'} [45, 'a', 9] (123, 321) <class 'tuple'>");
        BOOST_CHECK_EQUAL(lastBundleLoggingbStdOut, true);
    }

    BOOST_AUTO_TEST_CASE(test_simple_stderr) {
        auto bundleHash = generateUUID();

        auto testMessage = std::string{"'testing stdout'"};

        writeBundleLoggingStdErr(bundleHash, testMessage);

        auto result = BundleManager::Singleton()->runBundle_bool("logging_test", bundleHash, {}, "");

        BOOST_CHECK_EQUAL(result, true);
        BOOST_CHECK_EQUAL(lastBundleLoggingMessage, "Bundle [" + bundleHash + "]: testing stdout");
        BOOST_CHECK_EQUAL(lastBundleLoggingbStdOut, false);
    }

    BOOST_AUTO_TEST_CASE(test_complex_stderr) {
        auto bundleHash = generateUUID();

        auto testMessage = std::string{"'testing stdout', 56, {'a': 'b'}, [45, 'a', sum([5, 4])], (123, 321,), type((1,))"};

        writeBundleLoggingStdErr(bundleHash, testMessage);

        auto result = BundleManager::Singleton()->runBundle_bool("logging_test", bundleHash, {}, "");

        BOOST_CHECK_EQUAL(result, true);
        BOOST_CHECK_EQUAL(lastBundleLoggingMessage, "Bundle [" + bundleHash + "]: testing stdout 56 {'a': 'b'} [45, 'a', 9] (123, 321) <class 'tuple'>");
        BOOST_CHECK_EQUAL(lastBundleLoggingbStdOut, false);
    }
BOOST_AUTO_TEST_SUITE_END()