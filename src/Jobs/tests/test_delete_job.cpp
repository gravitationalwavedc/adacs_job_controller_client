//
// Created by lewis on 4/10/22.
//

#include "../JobHandling.h"
#include "../../DB/sStatus.h"
#include "../../Lib/JobStatus.h"
#include "../../Tests/fixtures/AbortHelperFixture.h"
#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"

void handleJobDeleteImpl(const std::shared_ptr<Message> &msg);

struct JobDeleteTestDataFixture
        : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture, public AbortHelperFixture, public TemporaryDirectoryFixture {
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    std::string bundleHash = generateUUID();
    sJob job;
    std::string tempDir = createTemporaryDirectory();

    JobDeleteTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}

        job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.schedulerId = 4321;
        job.bundleHash = bundleHash;
        job.running = false;
        job.workingDirectory = tempDir;
        job.save();
    }

    ~JobDeleteTestDataFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& /*connection*/) override {
        receivedMessages.push(message);
    }
};

BOOST_FIXTURE_TEST_SUITE(job_delete_test_suite, JobDeleteTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_delete_job_job_not_exists) {
        // Try to delete a job that doesn't exist
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId + 1);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job deletion");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId + 1);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::DELETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has been deleted");
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_job_running) {
        job.running = true;
        job.save();

        // Try to delete a job that is running
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.deleted, false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_job_submitting) {
        job.submitting = true;
        job.save();

        // Try to delete a job that is submitting
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.deleted, false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_job_deleted) {
        job.deleted = true;
        job.save();

        // Try to delete a job that is already deleted
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId + 1);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job deletion");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId + 1);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::DELETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has been deleted");
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_job_deleting) {
        job.deleting = true;
        job.save();

        // Try to delete a job that is already deleting
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.deleted, false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_failure) {
        writeJobDelete(bundleHash, job.schedulerId, job.jobId, "test_cluster", "False");

        // Try to delete a job where the bundle fails
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should not be deleting anymore after a failure, nor should it be deleted
        BOOST_CHECK_EQUAL(job.deleted, false);
        BOOST_CHECK_EQUAL(job.deleting, false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_delete_job_success) {
        writeJobDelete(bundleHash, job.schedulerId, job.jobId, "test_cluster", "True");

        // Try to delete a job that is submitting
        Message msg(DELETE_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job deletion");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::DELETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has been deleted");

        // Job should not be deleting anymore after a success, but it should be deleted
        BOOST_CHECK_EQUAL(job.deleted, true);
        BOOST_CHECK_EQUAL(job.deleting, false);
    }
BOOST_AUTO_TEST_SUITE_END()
