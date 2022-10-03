//
// Created by lewis on 9/28/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../Websocket/WebsocketInterface.h"
#include "../../lib/jobclient_schema.h"
#include "../../DB/SqliteConnector.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../tests/fixtures/BundleFixture.h"
#include "../JobHandling.h"
#include "../../tests/fixtures/DatabaseFixture.h"
#include "../../tests/fixtures/AbortHelperFixture.h"
#include "../../lib/JobStatus.h"

struct JobSubmitTestDataFixture : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture, public AbortHelperFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    std::string bundleHash = generateUUID();
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobSubmitTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    virtual ~JobSubmitTestDataFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(std::shared_ptr<TestWsServer::InMessage> message) {
        auto stringData = message->string();
        receivedMessages.push( std::make_shared<Message>(std::vector<uint8_t>(stringData.begin(), stringData.end())));
    }
};

BOOST_FIXTURE_TEST_SUITE(job_submit_test_suite, JobSubmitTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_submit_timeout) {
        auto job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.submitting = true;
        job.save();

        // Try to submit the job 10 times after marking the job as submitting
        for (int count = 1; count < 10; count++) {
            Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
            msg.push_uint(job.jobId);
            msg.push_string(bundleHash);
            msg.push_string("test params");
            msg.send(pWebsocketServerConnection);

            int retry = 0;
            for (; retry < 10 && job.submittingCount != count; retry++) {
                std::this_thread::sleep_for(std::chrono::milliseconds(100));
                job.refreshFromDb();
            }

            BOOST_CHECK_EQUAL(job.submittingCount, count);

            if (retry == 10) {
                BOOST_FAIL("Client never incremented submittingCount");
            }
        }

        // The next time the server tries to submit the job, the client should try to resubmit the job
        writeJobSubmit(bundleHash, "/a/test/working/directory/", "4321", 1234, "test params", "test_cluster");

        Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.push_string(bundleHash);
        msg.push_string("test params");
        msg.send(pWebsocketServerConnection);

        // Wait until job.workingDirectory is not empty, which indicates that the job has been "resubmitted"
        int retry = 0;
        for (; retry < 10 && job.workingDirectory == ""; retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        BOOST_CHECK_EQUAL(job.submittingCount, 0);
        BOOST_CHECK_EQUAL(job.jobId, 1234);
        BOOST_CHECK_EQUAL(job.workingDirectory, "/a/test/working/directory/");

        if (retry == 10) {
            BOOST_FAIL("Client never tried to resubmit the job");
        }

        // Next we wait for a response from the client
        retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never submitted the job to the bundle");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), SYSTEM_SOURCE);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::SUBMITTED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job submitted successfully");

        // Job should no longer be submitting, and should have a scheduler id set
        BOOST_CHECK_EQUAL(job.submitting, false);
        BOOST_CHECK_EQUAL(job.schedulerId, 4321);

        // The application should not have aborted
        BOOST_CHECK_EQUAL(checkAborted(), false);
    }

    BOOST_AUTO_TEST_CASE(test_submit_error_none) {
        // If the submit function returns 0 or None, then the job should be in an error state
        writeJobSubmitError(bundleHash, "return None");

        uint32_t jobId = 1234;

        Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(jobId);
        msg.push_string(bundleHash);
        msg.push_string("test params");
        msg.send(pWebsocketServerConnection);

        // Next we wait for two messages from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.size() != 2; retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never reported the job submission error");
        }

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), SYSTEM_SOURCE);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::ERROR);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Unable to submit job. Please check the logs as to why.");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::ERROR);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Unable to submit job. Please check the logs as to why.");

        for (int count = 0; count = 100; count++) {
            try {
                auto jobResults =
                        database->operator()(
                                select(all_of(jobTable))
                                        .from(jobTable)
                                        .where(
                                                jobTable.jobId == static_cast<uint64_t>(jobId)
                                        )
                        );

                if (!jobResults.empty()) {
                    BOOST_FAIL("There is a job record when there should not be.");
                } else {
                    break;
                }
            } catch (sqlpp::exception &except) {
                if (std::string(except.what()).find("database is locked") != std::string::npos) {
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        // The application should not have aborted
        BOOST_CHECK_EQUAL(checkAborted(), false);
    }

    BOOST_AUTO_TEST_CASE(test_submit_error_zero) {
        // If the submit function returns 0 or None, then the job should be in an error state
        writeJobSubmitError(bundleHash, "return 0");

        uint32_t jobId = 1234;

        Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(jobId);
        msg.push_string(bundleHash);
        msg.push_string("test params");
        msg.send(pWebsocketServerConnection);

        // Next we wait for two messages from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.size() != 2; retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never reported the job submission error");
        }

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), SYSTEM_SOURCE);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::ERROR);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Unable to submit job. Please check the logs as to why.");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::ERROR);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Unable to submit job. Please check the logs as to why.");

        for (int count = 0; count = 100; count++) {
            try {
                auto jobResults =
                        database->operator()(
                                select(all_of(jobTable))
                                        .from(jobTable)
                                        .where(
                                                jobTable.jobId == static_cast<uint64_t>(jobId)
                                        )
                        );

                if (!jobResults.empty()) {
                    BOOST_FAIL("There is a job record when there should not be.");
                } else {
                    break;
                }
            } catch (sqlpp::exception &except) {
                if (std::string(except.what()).find("database is locked") != std::string::npos) {
                    // Wait a small moment and try again
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                } else {
                    throw except;
                }
            }
        }

        // The application should not have aborted
        BOOST_CHECK_EQUAL(checkAborted(), false);
    }

    BOOST_AUTO_TEST_CASE(test_submit_error_abort) {
        // If the submit function returns 0 or None, then the job should be in an error state
        writeJobSubmitError(bundleHash, "assert False");

        uint32_t jobId = 1234;

        Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(jobId);
        msg.push_string(bundleHash);
        msg.push_string("test params");
        msg.send(pWebsocketServerConnection);

        // The client should abort due to the uncaught error in the bundle submit.
        int retry = 0;
        for (; retry < 10 && !checkAborted(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never aborted");
        }
    }

    BOOST_AUTO_TEST_CASE(test_submit_already_submitted) {
        auto job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.submitting = false;
        job.save();

        // Try to submit the job again, it should trigger a job status check
        Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.push_string(bundleHash);
        msg.push_string("test params");
        msg.send(pWebsocketServerConnection);

        BOOST_FAIL("checkJobStatus not implemented");

//        // The next time the server tries to submit the job, the client should try to resubmit the job
//        writeJobSubmit(bundleHash, "/a/test/working/directory/", "4321", 1234, "test params", "test_cluster");
//
//        msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
//        msg.push_uint(job.jobId);
//        msg.push_string(bundleHash);
//        msg.push_string("test params");
//        msg.send(pWebsocketServerConnection);
//
//        // Wait until job.workingDirectory is not empty, which indicates that the job has been "resubmitted"
//        int retry = 0;
//        for (; retry < 10 && job.workingDirectory == ""; retry++) {
//            std::this_thread::sleep_for(std::chrono::milliseconds(100));
//            job.refreshFromDb();
//        }
//
//        BOOST_CHECK_EQUAL(job.submittingCount, 0);
//        BOOST_CHECK_EQUAL(job.jobId, 1234);
//        BOOST_CHECK_EQUAL(job.workingDirectory, "/a/test/working/directory/");
//
//        if (retry == 10) {
//            BOOST_FAIL("Client never tried to resubmit the job");
//        }
//
//        // Next we wait for a response from the client
//        retry = 0;
//        for (; retry < 10 && receivedMessages.empty(); retry++) {
//            std::this_thread::sleep_for(std::chrono::milliseconds(100));
//        }
//
//        if (retry == 10) {
//            BOOST_FAIL("Client never submitted the job to the bundle");
//        }
//
//        // There should only have been one message indicating successful job submission
//        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);
//
//        // Check the message
//        job.refreshFromDb();
//
//        auto resultMsg = receivedMessages.front();
//        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
//        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
//        BOOST_CHECK_EQUAL(resultMsg->pop_string(), SYSTEM_SOURCE);
//        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::SUBMITTED);
//        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job submitted successfully");
//
//        // Job should no longer be submitting, and should have a scheduler id set
//        BOOST_CHECK_EQUAL(job.submitting, false);
//        BOOST_CHECK_EQUAL(job.schedulerId, 4321);
//
//        // The application should not have aborted
//        BOOST_CHECK_EQUAL(checkAborted(), false);
    }
BOOST_AUTO_TEST_SUITE_END()
