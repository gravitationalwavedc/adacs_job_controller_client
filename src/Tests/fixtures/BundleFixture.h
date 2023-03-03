//
// Created by lewis on 9/5/22.
//

#ifndef ADACS_JOB_CLIENT_BUNDLEFIXTURE_H
#define ADACS_JOB_CLIENT_BUNDLEFIXTURE_H

#include "../../Lib/GeneralUtils.h"
#include "../../Settings.h"
#include "nlohmann/json.hpp"
#include <boost/filesystem.hpp>
#include <fstream>

extern std::string fileListNoJobWorkingDirectoryScript;
extern std::string jobSubmitScript;
extern std::string jobSubmitErrorScript;
extern std::string jobCheckStatusScript;
extern std::string jobSubmitCheckStatusScript;
extern std::string bundleDbCreateOrUpdateJob;
extern std::string bundleDbGetJobById;
extern std::string bundleDbDeleteJob;
extern std::string jobCancelCheckStatusScript;
extern std::string jobDeleteScript;
extern std::string loggingStdOutScript;
extern std::string loggingStdErrScript;
extern std::string loggingStdOutDuringLoadScript;
extern std::string loggingStdErrDuringLoadScript;

class BundleFixture {
private:
  std::vector<std::string> cleanupPaths;

public:
  ~BundleFixture() {
      for (const auto &dir: cleanupPaths) {
          // Remove the directory and its contents.
          boost::filesystem::remove_all(dir);
      }
  }

  void writeFileListNoJobWorkingDirectory(const std::string &hash, const std::string &returnValue) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{fileListNoJobWorkingDirectoryScript};
      script.replace(script.find("xxx"), 3, returnValue);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeJobSubmit(const std::string &hash, const std::string &workingDirectory, const std::string &schedulerId,
                      uint64_t jobId, const std::string &params, const std::string &cluster) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{jobSubmitScript};
      script.replace(script.find("aaa"), 3, std::to_string(jobId));
      script.replace(script.find("bbb"), 3, cluster);
      script.replace(script.find("ccc"), 3, params);
      script.replace(script.find("ddd"), 3, schedulerId);
      script.replace(script.find("eee"), 3, workingDirectory);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeJobSubmitError(const std::string &hash, const std::string &resultLine) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{jobSubmitErrorScript};

      script.replace(script.find("xxx"), 3, resultLine);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeJobCheckStatus(const std::string &hash, const nlohmann::json &result, uint64_t jobId, uint64_t schedulerId,
                           const std::string &cluster) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{jobCheckStatusScript};

      script.replace(script.find("aaa"), 3, std::to_string(jobId));
      script.replace(script.find("bbb"), 3, std::to_string(schedulerId));
      script.replace(script.find("ccc"), 3, cluster);

      script.replace(script.find("xxx"), 3, result.dump());

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeJobSubmitCheckStatus(const std::string &hash, const std::string &workingDirectory,
                                 const std::string &schedulerId, uint64_t jobId, const std::string &params,
                                 const std::string &cluster, const nlohmann::json &statusResult) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{jobSubmitCheckStatusScript};

      script.replace(script.find("aaa"), 3, workingDirectory);
      script.replace(script.find("bbb"), 3, std::to_string(jobId));
      script.replace(script.find("bbb"), 3, std::to_string(jobId));
      script.replace(script.find("ccc"), 3, cluster);
      script.replace(script.find("ccc"), 3, cluster);
      script.replace(script.find("ddd"), 3, params);

      script.replace(script.find("ggg"), 3, schedulerId);
      script.replace(script.find("ggg"), 3, schedulerId);

      script.replace(script.find("iii"), 3, statusResult.dump());

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleDbCreateOrUpdateJob(const std::string &hash, const nlohmann::json &job) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{bundleDbCreateOrUpdateJob};

      script.replace(script.find("xxx"), 3, job.dump());

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleDbGetJobById(const std::string &hash, uint64_t jobId) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{bundleDbGetJobById};

      script.replace(script.find("xxx"), 3, std::to_string(jobId));

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleDbDeleteJob(const std::string &hash, const nlohmann::json &job) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{bundleDbDeleteJob};

      script.replace(script.find("xxx"), 3, job.dump());

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void
  writeJobCancelCheckStatus(const std::string &hash, uint64_t schedulerId, uint64_t jobId, const std::string &cluster,
                            const nlohmann::json &statusResult, const std::string &cancelResult) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{jobCancelCheckStatusScript};

      script.replace(script.find("bbb"), 3, std::to_string(jobId));
      script.replace(script.find("bbb"), 3, std::to_string(jobId));
      script.replace(script.find("ccc"), 3, cluster);
      script.replace(script.find("ccc"), 3, cluster);
      script.replace(script.find("ddd"), 3, cancelResult);

      script.replace(script.find("ggg"), 3, std::to_string(schedulerId));
      script.replace(script.find("ggg"), 3, std::to_string(schedulerId));

      script.replace(script.find("iii"), 3, statusResult.dump());

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeJobDelete(const std::string &hash, uint64_t schedulerId, uint64_t jobId, const std::string &cluster,
                      const std::string &deleteResult) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{jobDeleteScript};

      script.replace(script.find("bbb"), 3, std::to_string(jobId));
      script.replace(script.find("ccc"), 3, cluster);
      script.replace(script.find("ddd"), 3, deleteResult);

      script.replace(script.find("ggg"), 3, std::to_string(schedulerId));

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleLoggingStdOut(const std::string &hash, const std::string &content) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{loggingStdOutScript};

      script.replace(script.find("xxx"), 3, content);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleLoggingStdErr(const std::string &hash, const std::string &content) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{loggingStdErrScript};

      script.replace(script.find("xxx"), 3, content);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleLoggingStdOutDuringLoad(const std::string &hash, const std::string &content) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{loggingStdOutDuringLoadScript};

      script.replace(script.find("xxx"), 3, content);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }

  void writeBundleLoggingStdErrDuringLoad(const std::string &hash, const std::string &content) {
      auto path = boost::filesystem::path(getBundlePath()) / hash;
      boost::filesystem::create_directories(path);
      cleanupPaths.push_back(path.string());

      auto script = std::string{loggingStdErrDuringLoadScript};

      script.replace(script.find("xxx"), 3, content);

      std::ofstream ostr((path / "bundle.py").string());
      ostr << script;
      ostr.close();
  }
};

#endif //ADACS_JOB_CLIENT_BUNDLEFIXTURE_H
