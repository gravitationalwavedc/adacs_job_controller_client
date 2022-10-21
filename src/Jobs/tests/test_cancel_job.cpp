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

void handleJobCancelImpl(const std::shared_ptr<Message> &msg);

struct JobCancelTestDataFixture
        : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture, public AbortHelperFixture, public TemporaryDirectoryFixture {
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    std::string bundleHash = generateUUID();
    sJob job;
    std::string symlinkDir = createTemporaryDirectory();
    std::string symlinkFile = createTemporaryFile(symlinkDir);
    std::string tempDir = createTemporaryDirectory();
    std::string tempDir10 = createTemporaryDirectory();
    std::string tempDir11 = createTemporaryDirectory();
    std::string tempFile = createTemporaryFile(tempDir);
    std::string tempDir2 = createTemporaryDirectory(tempDir);
    std::string tempFile2 = createTemporaryFile(tempDir2);

    JobCancelTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}

        // Create symlinks
        boost::filesystem::create_directory_symlink(symlinkDir, boost::filesystem::path(tempDir) / "symlink_dir");
        boost::filesystem::create_symlink(symlinkFile, boost::filesystem::path(tempDir) / "symlink_path");
        boost::filesystem::create_directory_symlink("/not/a/real/path", boost::filesystem::path(tempDir) / "symlink_dir_not_real");
        boost::filesystem::create_symlink("/not/a/real/path", boost::filesystem::path(tempDir) / "symlink_path_not_real");

        // Generate some fake data
        std::ofstream ofs1(tempFile);
        ofs1 << "12345";
        ofs1.close();

        std::ofstream ofs2(tempFile2);
        ofs2 << "12345678";
        ofs2.close();

        job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.schedulerId = 4321;
        job.bundleHash = bundleHash;
        job.running = true;
        job.workingDirectory = tempDir;
        job.save();
    }

    ~JobCancelTestDataFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& /*connection*/) override {
        receivedMessages.push(message);
    }
};

BOOST_FIXTURE_TEST_SUITE(job_cancel_test_suite, JobCancelTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_cancel_job_job_not_exists) {
        // Try to cancel a job that doesn't exist
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId + 1);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job cancellation");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId + 1);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::CANCELLED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has been cancelled");

        // Job archive should not have been generated
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);
    }

    BOOST_AUTO_TEST_CASE(test_cancel_job_job_not_running) {
        job.running = false;
        job.save();

        // Try to cancel a job that is not running
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job cancellation");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::CANCELLED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has been cancelled");

        // Job archive should not have been generated
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);
    }

    BOOST_AUTO_TEST_CASE(test_cancel_job_job_submitting) {
        job.submitting = true;
        job.save();

        // Try to cancel a job that is submitting
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_cancel_job_not_running_after_status_check) {
        nlohmann::json result = {
                {
                        "status",   nlohmann::json::array(
                        {

                        }
                )
                },
                {"complete", true}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Try to cancel a job that is initially running, but is not running after a status check
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job cancellation");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has completed");

        // Job should no longer be running, and the archive.tar.gz should have been generated
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);
    }

    BOOST_AUTO_TEST_CASE(test_cancel_job_running_after_status_check_cancel_error) {
        nlohmann::json result = {
                {
                        "status",   nlohmann::json::array(
                        {

                        }
                )
                },
                {"complete", false}
        };
        writeJobCancelCheckStatus(bundleHash, job.schedulerId, job.jobId, "test_cluster", result, "False");

        // Try to cancel a job that is initially running, but is not running after a status check
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_cancel_job_running_after_status_check_cancel_success_already_cancelled) {
        nlohmann::json result = {
                {
                        "status",   nlohmann::json::array(
                        {

                        }
                )
                },
                {"complete", false}
        };
        writeJobCancelCheckStatus(bundleHash, job.schedulerId, job.jobId, "test_cluster", result, "True");

        sStatus status = {
                .jobId = job.jobId,
                .what = "anything",
                .state = JobStatus::CANCELLED
        };

        status.save();

        // Try to cancel a job that is initially running, but is not running after a status check
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait a short moment then confirm the job is still running
        std::this_thread::sleep_for(std::chrono::milliseconds(500));

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // No messages should have been sent
        BOOST_CHECK_EQUAL(receivedMessages.empty(), true);
    }

    BOOST_AUTO_TEST_CASE(test_cancel_job_running_after_status_check_cancel_success_not_already_cancelled) {
        nlohmann::json result = {
                {
                        "status",   nlohmann::json::array(
                        {

                        }
                )
                },
                {"complete", false}
        };
        writeJobCancelCheckStatus(bundleHash, job.schedulerId, job.jobId, "test_cluster", result, "True");

        // Try to cancel a job that is initially running, but is not running after a status check
        Message msg(CANCEL_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.send(pWebsocketServerConnection);

        // Wait until we receive a message back from the server
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job cancellation");
        }

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::CANCELLED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has been cancelled");

        // Check the message
        job.refreshFromDb();

        // Job should still be running, and the archive.tar.gz should not have been generated
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);
    }
BOOST_AUTO_TEST_SUITE_END()
