//
// Created by lewis on 9/28/22.
//

#include "../JobHandling.h"
#include "../../DB/sStatus.h"
#include "../../Lib/JobStatus.h"
#include "../../Tests/fixtures/AbortHelperFixture.h"
#include "../../Tests/fixtures/BundleFixture.h"
#include "../../Tests/fixtures/TemporaryDirectoryFixture.h"
#include "../../Tests/fixtures/WebsocketServerFixture.h"

struct JobSubmitTestDataFixture : public WebsocketServerFixture, public BundleFixture, public AbortHelperFixture, public TemporaryDirectoryFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    std::queue<std::shared_ptr<Message>> receivedMessages;
    std::string bundleHash = generateUUID();

    // Create temporary files and directories
    std::string symlinkDir = createTemporaryDirectory();
    std::string symlinkFile = createTemporaryFile(symlinkDir);
    std::string tempDir = createTemporaryDirectory();
    std::string tempFile = createTemporaryFile(tempDir);
    std::string tempDir2 = createTemporaryDirectory(tempDir);
    std::string tempFile2 = createTemporaryFile(tempDir2);
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobSubmitTestDataFixture() {
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
    }

    ~JobSubmitTestDataFixture() override {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& /*connection*/) override {
        receivedMessages.push(message);
    }
};

BOOST_FIXTURE_TEST_SUITE(job_submit_test_suite, JobSubmitTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_submit_timeout) { // NOLINT(readability-function-cognitive-complexity)
        auto job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.submitting = true;
        job.save();

        // Try to submit the job 10 times after marking the job as submitting
        for (int count = 1; count < MAX_SUBMIT_COUNT; count++) {
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
        for (; retry < 10 && job.workingDirectory.empty(); retry++) {
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

    BOOST_AUTO_TEST_CASE(test_submit_error_none) { // NOLINT(readability-function-cognitive-complexity)
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

        for (int count = 0; count < 100; count++) {
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

    BOOST_AUTO_TEST_CASE(test_submit_error_zero) { // NOLINT(readability-function-cognitive-complexity)
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

        for (int count = 0; count < 100; count++) {
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

    BOOST_AUTO_TEST_CASE(test_submit_already_submitted) {
        auto job = sJob::getOrCreateByJobId(1234);
        job.jobId = 1234;
        job.schedulerId = 4321;
        job.submitting = false;
        job.running = true;
        job.workingDirectory = tempDir;
        job.bundleHash = bundleHash;
        job.save();

        nlohmann::json result = {
                {
                        "status", nlohmann::json::array({})
                },
                {"complete", true}
        };

        // The next time the server tries to submit the job, the client should try to resubmit the job
        writeJobSubmitCheckStatus(bundleHash, job.workingDirectory, "4321", 1234, "test params", "test_cluster", result);

        // Try to submit the job again, it should trigger a job status check
        Message msg(SUBMIT_JOB, Message::Priority::Medium, SYSTEM_SOURCE);
        msg.push_uint(job.jobId);
        msg.push_string(bundleHash);
        msg.push_string("test params");
        msg.send(pWebsocketServerConnection);

        // Wait until job.workingDirectory is not empty, which indicates that the job has been "resubmitted"
        int retry = 0;
        for (; retry < 10 && receivedMessages.empty(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            job.refreshFromDb();
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        // There should only have been one message indicating successful job submission
        BOOST_CHECK_EQUAL(receivedMessages.size(), 1);

        // Check the message
        auto resultMsg = receivedMessages.front();
        BOOST_CHECK_EQUAL(resultMsg->getId(), UPDATE_JOB);
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), job.jobId);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "_job_completion_");
        BOOST_CHECK_EQUAL(resultMsg->pop_uint(), JobStatus::COMPLETED);
        BOOST_CHECK_EQUAL(resultMsg->pop_string(), "Job has completed");

        // There should be no status records since no status changes occurred
        BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.id).size(), 0);

        // Job should be finished with archive
        BOOST_CHECK_EQUAL(job.running, false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);

        // There should be one message indicating that the job has completed
    }
BOOST_AUTO_TEST_SUITE_END()
