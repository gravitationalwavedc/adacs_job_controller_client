//
// Created by lewis on 9/28/22.
//

#ifndef ADACS_JOB_CLIENT_TEMPORARYDIRECTORYFIXTURE_H
#define ADACS_JOB_CLIENT_TEMPORARYDIRECTORYFIXTURE_H

#include <vector>
#include <string>
#include <boost/filesystem.hpp>

class TemporaryDirectoryFixture {
private:
    std::vector<std::string> tempDirs;

public:
    std::string createTemporaryDirectory() {
        boost::filesystem::path ph = boost::filesystem::temp_directory_path() / boost::filesystem::unique_path();
        boost::filesystem::create_directories(ph);

        tempDirs.push_back(ph.string());

        return ph.string();
    }

    std::string createTemporaryFile(std::string parent = "") {
        boost::filesystem::path ph =
                boost::filesystem::path(parent == "" ? boost::filesystem::temp_directory_path() : parent)
                / boost::filesystem::unique_path();

        boost::filesystem::create_directories(ph.parent_path());

        std::ofstream ofs(ph.string());
        ofs.close();

        tempDirs.push_back(ph.string());

        return ph.string();
    }

    ~TemporaryDirectoryFixture() {
        for (const auto& dir : tempDirs) {
            // Remove the directory and its contents.
            boost::filesystem::remove_all(dir);
        }
    }
};

#endif //ADACS_JOB_CLIENT_TEMPORARYDIRECTORYFIXTURE_H
