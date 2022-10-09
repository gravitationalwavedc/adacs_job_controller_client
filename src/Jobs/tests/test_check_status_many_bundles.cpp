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

struct JobCheckStatusManyBundlesTestDataFixture
        : public WebsocketServerFixture, public BundleFixture, public DatabaseFixture, public AbortHelperFixture, public TemporaryDirectoryFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string token;
    uint64_t receivedMessages = 0;
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobCheckStatusManyBundlesTestDataFixture() {
        token = generateUUID();
        WebsocketInterface::setSingleton(nullptr);
        WebsocketInterface::setSingleton(std::make_shared<WebsocketInterface>(token));

        startWebSocketServer();

        WebsocketInterface::Singleton()->start();
        websocketServerConnectionPromise.get_future().wait();
        while (!*WebsocketInterface::Singleton()->getpConnection()) {}
    }

    std::string generateWorkingDirectory() {
        // Create temporary files and directories
        std::string symlinkDir = createTemporaryDirectory();
        std::string symlinkFile = createTemporaryFile(symlinkDir);
        std::string tempDir = createTemporaryDirectory();
        std::string tempFile = createTemporaryFile(tempDir);
        std::string tempDir2 = createTemporaryDirectory(tempDir);
        std::string tempFile2 = createTemporaryFile(tempDir2);

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

        return tempDir;
    }

    sJob generateJob(uint64_t index, std::string bundleHash, std::string workingDirectory) {
        auto job = sJob::getOrCreateByJobId(1234 + index);
        job.jobId = 1234 + index;
        job.schedulerId = 432100 + index;
        job.bundleHash = bundleHash;
        job.running = true;
        job.workingDirectory = workingDirectory;
        job.save();

        return job;
    }

    virtual ~JobCheckStatusManyBundlesTestDataFixture() {
        WebsocketInterface::Singleton()->stop();
    }

    void onWebsocketServerMessage(const std::shared_ptr<Message>& message, const std::shared_ptr<TestWsServer::Connection>& connection) {
        receivedMessages++;
    }
};

BOOST_FIXTURE_TEST_SUITE(job_check_status_many_bundles_test_suite, JobCheckStatusManyBundlesTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_check_all_job_status) {
        nlohmann::json result = {
                {
                        "status", nlohmann::json::array({})
                },
                {"complete", true}
        };

        struct sBundleDetails {
            std::string bundleHash;
            sJob job;
        };

        std::vector<sBundleDetails> jobs;

        for (int bundleCount = 0; bundleCount < 100; bundleCount++) {
            auto workingDirectory = generateWorkingDirectory();
            auto bundleHash = generateUUID();
            sBundleDetails bundleDetails {
                .bundleHash = bundleHash,
                .job = generateJob(bundleCount, bundleHash, workingDirectory)
            };

            writeJobCheckStatus(bundleDetails.bundleHash, result, 1234 + bundleCount, 432100 + bundleCount, "test_cluster");

            jobs.push_back(bundleDetails);
        }

        checkAllJobsStatus();

        for (auto job : jobs) {
            // There should be no status records since no status changes occurred
            BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.job.id).size(), 0);

            // All jobs should be finished with archives
            job.job.refreshFromDb();
            BOOST_CHECK_EQUAL(job.job.running, false);
            BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.job.workingDirectory) / "archive.tar.gz"), true);
        }

        // 100 messages should have been sent to notify the server of job completion
        int retry = 0;
        for (; retry < 10 && receivedMessages != jobs.size(); retry++) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }

        if (retry == 10) {
            BOOST_FAIL("Client never notified the server of job status change");
        }

        BOOST_CHECK_EQUAL(receivedMessages, jobs.size());
    }

BOOST_AUTO_TEST_SUITE_END()
