//
// Created by lewis on 4/10/22.
//

#include "JobHandling.h"
#include "subprocess.hpp"

auto archiveJob(const sJob& job) -> bool {
    /*
    Archives the output directory of a job in a tar.gz file
    Args:
        job: The job to archive

    Returns:
        Nothing
    */

    // Attempt to tar up the job using the tar utility
    auto proc = subprocess::Popen(
            {"tar", "-cvf", "archive.tar.gz", "."},
            subprocess::output{subprocess::PIPE},
            subprocess::error{subprocess::PIPE},
            subprocess::shell{false},
            subprocess::cwd{job.workingDirectory}
    );

    auto communication = proc.communicate();
    auto obuf = communication.first;
    auto ebuf = communication.second;

    std::string sOut(obuf.buf.begin(), obuf.buf.end());
    std::string sErr(ebuf.buf.begin(), ebuf.buf.end());
    std::cout << "Archiving job " << job.jobId << " completed with code " << proc.retcode()
    << std::endl << std::endl << "Stdout: " << sOut
    << std::endl << std::endl << "Stderr: " << sErr << std::endl;

    return proc.retcode() == 0;
}