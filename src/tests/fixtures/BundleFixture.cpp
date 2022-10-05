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