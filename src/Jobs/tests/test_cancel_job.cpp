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

void handleJobCancelImpl(const std::shared_ptr<Message> &msg)

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

        handleJobCancelImpl(job);

        // This test should result in a no-op, there are no status changes, and the job is not complete
        BOOST_CHECK_EQUAL(sStatus::getJobStatusByJobId(job.id).size(), 0);

        // Job should still be running, and the archive should not have been generated
        job.refreshFromDb();
        BOOST_CHECK_EQUAL(job.running, true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);
    }
BOOST_AUTO_TEST_SUITE_END()
