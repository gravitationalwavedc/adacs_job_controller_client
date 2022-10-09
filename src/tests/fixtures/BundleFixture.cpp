//
// Created by lewis on 9/30/22.
//

#include <string>

std::string fileListNoJobWorkingDirectoryScript = R"PY(
def working_directory(details, job_data):
    return "xxx"
)PY";

std::string jobSubmitScript = R"PY(
def working_directory(details, job_data):
    return "eee"

def submit(details, job_data):
    assert details["job_id"] == aaa
    assert details["cluster"] == "bbb"
    assert job_data == "ccc"
    return ddd
)PY";

std::string jobSubmitErrorScript = R"PY(
def working_directory(details, job_data):
    return "/doesnt/matter/"

def submit(details, job_data):
    xxx
)PY";

std::string jobCheckStatusScript = R"PY(
def status(details, job_data):
    assert details["job_id"] == aaa
    assert details["scheduler_id"] == bbb
    assert details["cluster"] == "ccc"
    import json
    return json.loads("""xxx""")
)PY";

std::string jobSubmitCheckStatusScript = R"PY(
def working_directory(details, job_data):
    return "aaa"

def submit(details, job_data):
    assert details["job_id"] == bbb
    assert details["cluster"] == "ccc"
    assert job_data == "ddd"
    return ggg

def status(details, job_data):
    assert details["job_id"] == bbb
    assert details["scheduler_id"] == ggg
    assert details["cluster"] == "ccc"
    import json
    return json.loads("""iii""")
)PY";

std::string bundleDbCreateOrUpdateJob = R"PY(
import _bundledb
import json

def submit(details, job_data):
    job = json.loads("""xxx""")
    _bundledb.create_or_update_job(job)
    return job
)PY";