//
// Created by lewis on 9/5/22.
//

    #ifndef ADACS_JOB_CLIENT_BUNDLEFIXTURE_H
    #define ADACS_JOB_CLIENT_BUNDLEFIXTURE_H

#include <fstream>
#include "nlohmann/json.hpp"
#include "../../Settings.h"
#include "../../lib/GeneralUtils.h"
#include <boost/filesystem.hpp>

std::string fileListNoJobWorkingDirectoryScript = R"PY(
def working_directory(details, job_data):
    return "xxx"
)PY";

class BundleFixture {
private:
    std::vector<std::string> cleanupPaths;

public:
    ~BundleFixture() {
        for (const auto& dir : cleanupPaths) {
            // Remove the directory and its contents.
            boost::filesystem::remove_all(dir);
        }
    }

    void writeFileListNoJobWorkingDirectory(const std::string& hash, const std::string& returnValue) {
        auto path = boost::filesystem::path(getBundlePath()) / hash;
        boost::filesystem::create_directories(path);
        cleanupPaths.push_back(path.string());

        auto script = std::string{fileListNoJobWorkingDirectoryScript};
        script.replace(script.find("xxx"), 3, returnValue);

        std::ofstream ostr((path / "bundle.py").string());
        ostr << script;
        ostr.close();
    }
};

#endif //ADACS_JOB_CLIENT_BUNDLEFIXTURE_H
