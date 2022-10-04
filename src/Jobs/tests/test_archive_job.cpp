//
// Created by lewis on 4/10/22.
//

#include "../../tests/fixtures/WebsocketServerFixture.h"
#include "../../lib/jobclient_schema.h"
#include "../../tests/fixtures/TemporaryDirectoryFixture.h"
#include "../JobHandling.h"
#include <boost/filesystem.hpp>


struct JobArchiveTestDataFixture : public TemporaryDirectoryFixture {
    // NOLINTBEGIN(misc-non-private-member-variables-in-classes)
    std::string symlinkDir = createTemporaryDirectory();
    std::string symlinkFile = createTemporaryFile(symlinkDir);
    std::string tempDir = createTemporaryDirectory();
    std::string tempFile = createTemporaryFile(tempDir);
    std::string tempDir2 = createTemporaryDirectory(tempDir);
    std::string tempFile2 = createTemporaryFile(tempDir2);
    // NOLINTEND(misc-non-private-member-variables-in-classes)

    JobArchiveTestDataFixture() {
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
};

BOOST_FIXTURE_TEST_SUITE(job_archive_test_suite, JobArchiveTestDataFixture)
    BOOST_AUTO_TEST_CASE(test_archive_success) {
        sJob job = {
                .workingDirectory = tempDir
        };

        BOOST_CHECK_EQUAL(archiveJob(job), true);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), true);
    }

    BOOST_AUTO_TEST_CASE(test_archive_fail) {
        sJob job = {
                .workingDirectory = "/"
        };

        BOOST_CHECK_EQUAL(archiveJob(job), false);
        BOOST_CHECK_EQUAL(boost::filesystem::exists(boost::filesystem::path(job.workingDirectory) / "archive.tar.gz"), false);
    }
BOOST_AUTO_TEST_SUITE_END()