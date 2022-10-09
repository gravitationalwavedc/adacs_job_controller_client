//
// Created by lewis on 4/10/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../Websocket/WebsocketInterface.h"
#include "../../lib/jobclient_schema.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../tests/fixtures/BundleFixture.h"
#include "../JobHandling.h"
#include "../../tests/fixtures/DatabaseFixture.h"
#include "../../tests/fixtures/AbortHelperFixture.h"
#include "../../lib/JobStatus.h"
#include "../../DB/sStatus.h"

void checkJobStatusImpl(sJob job, bool forceNotification);

struct JobCheckStatusTestDataFixture
        : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture, public AbortHelperFixture, public TemporaryDirectoryFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
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
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobCheckStatusTestDataFixture() {
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

    virtual ~JobCheckStatusTestDataFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& connection) {
        receivedMessages.push(message);
    }
};

BOOST_FIXTURE_TEST_SUITE(job_check_status_test_suite, JobCheckStatusTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_check_status_no_status_not_complete) {
        nlohmann::json result = {
                {
                        "status",   nlohmann::json::array(
                        {

                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, true);

        // This test should result in a no-op, there are no status changes, and the job is not complete
        BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.id).size(), 0);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);
    }

    BOOST_AUTO_TEST_CASE(test_check_status_no_status_complete) {
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

        checkJobStatusImpl(job, true);

        // There should be no status changes
        BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.id).size(), 0);

        // The client should have archived the job, and updated the server that the job has completed successfully
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job completion");
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

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_force_notification_duplicates) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create several duplicate job status records
        sStatus status = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::QUEUED
        };

        status.save();
        status.id = 0;
        status.save();

        checkJobStatusImpl(job, true);

        // The duplicate job status records should have been deleted, and replaced with a new status record
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 1);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_force_notification) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create a job status record
        sStatus status = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::QUEUED
        };

        status.save();

        checkJobStatusImpl(job, true);

        // One status record should exist, that has the same id as the original status because it should have been updated
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 1);
        BOOST_CHECK_EQUAL(vStatus[0].id, status.id);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_force_notification_same_status) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create a job status record
        sStatus status = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::RUNNING
        };

        status.save();

        checkJobStatusImpl(job, true);

        // One status record should exist, that has the same id as the original status because it should have been updated
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 1);
        BOOST_CHECK_EQUAL(vStatus[0].id, status.id);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_same_status) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create a job status record
        sStatus status = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::RUNNING
        };

        status.save();

        checkJobStatusImpl(job, false);

        // One status record should exist, that has the same id as the original status because it should have been updated
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 1);
        BOOST_CHECK_EQUAL(vStatus[0].id, status.id);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry != 10) {
            BOOST_FAIL("Client notified the server of job status change");
        }

        // There should not have been any messages sent if the status has not changed and not forcing notifications
        BOOST_CHECK_EQUAL(receivedMessages.size(), 0);
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_new_status) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, false);

        // One status record should exist that should have been created
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 1);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_changed_status) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create a job status record
        sStatus status = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::QUEUED
        };

        status.save();

        checkJobStatusImpl(job, false);

        // One status record should exist, that has the same id as the original status because it should have been updated
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 1);
        BOOST_CHECK_EQUAL(vStatus[0].id, status.id);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_same_status_multiple) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::COMPLETED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create a job status record
        sStatus status1 = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::COMPLETED
        };
        status1.save();

        sStatus status2 = {
                .jobId = job.id,
                .what = "test_what2",
                .state = JobStatus::RUNNING
        };
        status2.save();

        checkJobStatusImpl(job, false);

        // One status record should exist, that has the same id as the original status because it should have been updated
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].id, status1.id);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(vStatus[1].id, status2.id);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry != 10) {
            BOOST_FAIL("Client notified the server of job status change");
        }

        // There should not have been any messages sent if the status has not changed and not forcing notifications
        BOOST_CHECK_EQUAL(receivedMessages.size(), 0);
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_new_status_multiple) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::COMPLETED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, false);

        // One status record should exist that should have been created
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 2);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what2");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_changed_status_multiple) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::COMPLETED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::RUNNING}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Create a job status record
        sStatus status1 = {
                .jobId = job.id,
                .what = "test_what",
                .state = JobStatus::RUNNING
        };
        status1.save();

        sStatus status2 = {
                .jobId = job.id,
                .what = "test_what2",
                .state = JobStatus::QUEUED
        };
        status2.save();

        checkJobStatusImpl(job, false);

        // One status record should exist, that has the same id as the original status because it should have been updated
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].id, status1.id);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(vStatus[1].id, status2.id);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::RUNNING);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 2);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what2");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::RUNNING);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_error_1) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::COMPLETED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::ERROR}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, false);

        // One status record should exist that should have been created
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::ERROR);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 3);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what2");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::ERROR);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::ERROR);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has completed");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_running_error_2) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::WALL_TIME_EXCEEDED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::COMPLETED}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, false);

        // One status record should exist that should have been created
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::WALL_TIME_EXCEEDED);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::COMPLETED);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 3);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::WALL_TIME_EXCEEDED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what2");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::WALL_TIME_EXCEEDED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has completed");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_steps_complete_no_bundle_complete) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::COMPLETED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::COMPLETED}
                                }
                        }
                )
                },
                {"complete", false}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, false);

        // One status record should exist that should have been created
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::COMPLETED);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 2);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what2");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_job_steps_complete_bundle_complete) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array(
                        {
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what"},
                                        {"status", JobStatus::COMPLETED}
                                },
                                nlohmann::json{
                                        {"info", "Some info"},
                                        {"what", "test_what2"},
                                        {"status", JobStatus::COMPLETED}
                                }
                        }
                )
                },
                {"complete", true}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        checkJobStatusImpl(job, false);

        // One status record should exist that should have been created
        auto vStatus = sStatus::getJobStatusByJobId(job.id);
        BOOST_CHECK_EQUAL(vStatus.size(), 2);
        BOOST_CHECK_EQUAL(vStatus[0].what, "test_what");
        BOOST_CHECK_EQUAL(vStatus[0].state, JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(vStatus[1].what, "test_what2");
        BOOST_CHECK_EQUAL(vStatus[1].state, JobStatus::COMPLETED);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);

        // The client should have sent a new status update message to indicate the job is running
        // Next we wait for a response from the client
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 3);

        // Check the message
        job.refreshFromDb();

        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "test_what2");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Some info");

        receivedMessages.pop();
        resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has completed");
    }

    BOOST_AUTO_TEST_CASE(test_check_status_exception) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array({})
                },
                {"complete", true}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "raise_exception");

        checkJobStatus(job, false).join();

        // This test should result in a no-op, there are no status changes, and the job is not complete
        BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.id).size(), 0);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);
    }

    BOOST_AUTO_TEST_CASE(test_check_all_job_status) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array({})
                },
                {"complete", true}
        };
        writeJobCheckStatus(bundleHash, result, 1234, 4321, "test_cluster");

        // Clone the job a couple of times
        auto job2 = job;
        job2.id = 0;
        job2.workingDirectory = tempDir10;
        job2.save();

        auto job3 = job;
        job3.id = 0;
        job3.workingDirectory = tempDir11;
        job3.save();

        checkAllJobsStatus();

        // There should be no status records since no status changes occurred
        BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.id).size(), 0);

        // All jobs should be finished with archives
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);

        job2.refreshFromDb();
        BOOST_CHECK_EQUAL(job2.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job2.workingDirectory) / "archive.tar.gz"), true);

        job3.refreshFromDb();
        BOOST_CHECK_EQUAL(job3.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job3.workingDirectory) / "archive.tar.gz"), true);
    }

BOOST_AUTO_TEST_SUITE_END()
