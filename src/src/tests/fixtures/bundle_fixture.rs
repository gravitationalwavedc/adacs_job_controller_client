use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

pub const LOGGING_STDOUT_SCRIPT: &str = r"
def logging_test(details, job_data):
    print(xxx)
    return True
";

pub const LOGGING_STDERR_SCRIPT: &str = r"
def logging_test(details, job_data):
    import sys
    print(xxx, file=sys.stderr)
    return True
";

pub const LOGGING_STDOUT_DURING_LOAD_SCRIPT: &str = r"
print(xxx)
def logging_test(details, job_data):
    return True
";

pub const LOGGING_STDERR_DURING_LOAD_SCRIPT: &str = r"
import sys
print(xxx, file=sys.stderr)
def logging_test(details, job_data):
    return True
";

pub const JOB_SUBMIT_SCRIPT: &str = r#"
def working_directory(details, job_data):
    return "WORKING_DIR"

def submit(details, job_data):
    return SCHEDULER_ID
"#;

pub const JOB_SUBMIT_ERROR_SCRIPT: &str = r#"
def working_directory(details, job_data):
    return "WORKING_DIR"

def submit(details, job_data):
    return ERROR_RETURN
"#;

pub const JOB_STATUS_SCRIPT: &str = r#"
def working_directory(details, job_data):
    return "WORKING_DIR"

def submit(details, job_data):
    return SCHEDULER_ID

def status(details, job_data):
    import json
    return json.loads('STATUS_JSON')
"#;

pub struct BundleFixture {
    pub temp_dir: TempDir,
}

impl BundleFixture {
    pub fn new() -> Self {
        Self {
            temp_dir: TempDir::new().expect("Failed to create temp dir"),
        }
    }

    pub fn get_bundle_path(&self) -> PathBuf {
        self.temp_dir.path().to_path_buf()
    }

    fn write_script(&self, hash: &str, script: &str, replacements: &[(&str, &str)]) {
        let bundle_path = self.get_bundle_path().join(hash);
        fs::create_dir_all(&bundle_path).expect("Failed to create bundle directory");

        let mut script_content = script.to_string();
        for (old, new) in replacements {
            script_content = script_content.replace(old, new);
        }
        fs::write(bundle_path.join("bundle.py"), script_content)
            .expect("Failed to write bundle.py");
    }

    pub fn write_bundle_logging_std_out(&self, hash: &str, content: &str) {
        self.write_script(hash, LOGGING_STDOUT_SCRIPT, &[("xxx", content)]);
    }

    pub fn write_bundle_logging_std_err(&self, hash: &str, content: &str) {
        self.write_script(hash, LOGGING_STDERR_SCRIPT, &[("xxx", content)]);
    }

    pub fn write_bundle_logging_std_out_during_load(&self, hash: &str, content: &str) {
        self.write_script(hash, LOGGING_STDOUT_DURING_LOAD_SCRIPT, &[("xxx", content)]);
    }

    pub fn write_bundle_logging_std_err_during_load(&self, hash: &str, content: &str) {
        self.write_script(hash, LOGGING_STDERR_DURING_LOAD_SCRIPT, &[("xxx", content)]);
    }

    pub fn write_raw_script(&self, hash: &str, content: &str) {
        fs::create_dir_all(self.temp_dir.path().join(hash)).unwrap();
        fs::write(self.temp_dir.path().join(hash).join("bundle.py"), content).unwrap();
    }

    pub fn write_job_submit(&self, hash: &str, working_dir: &str, scheduler_id: &str) {
        self.write_script(
            hash,
            JOB_SUBMIT_SCRIPT,
            &[("WORKING_DIR", working_dir), ("SCHEDULER_ID", scheduler_id)],
        );
    }

    pub fn write_job_submit_error(&self, hash: &str, error_return: &str) {
        self.write_script(
            hash,
            JOB_SUBMIT_ERROR_SCRIPT,
            &[("ERROR_RETURN", error_return)],
        );
    }

    pub fn write_job_cancel(&self, hash: &str, return_val: &str) {
        let script = format!(
            r"
def cancel(details, job_data):
    return {return_val}
"
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_job_delete(&self, hash: &str, return_val: &str) {
        let script = format!(
            r"
def delete(details, job_data):
    return {return_val}
"
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_job_submit_check_status(
        &self,
        hash: &str,
        working_dir: &str,
        scheduler_id: &str,
        status_json: &str,
    ) {
        self.write_script(
            hash,
            JOB_STATUS_SCRIPT,
            &[
                ("WORKING_DIR", working_dir),
                ("SCHEDULER_ID", scheduler_id),
                ("STATUS_JSON", status_json),
            ],
        );
    }

    pub fn write_job_status(&self, hash: &str, status_json: &str) {
        let script = format!(
            r"
def status(details, job_data):
    import json
    return json.loads('{status_json}')
"
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_bundle_db_create_or_update_job(&self, hash: &str, job_json: &str) {
        let script = format!(
            r#"
import _bundledb
import json

def submit(details, job_data):
    job = json.loads('{job_json}')
    try:
        _bundledb.create_or_update_job(job)
        return job
    except Exception as e:
        return {{"error": str(e)}}
"#
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_bundle_db_get_job_by_id(&self, hash: &str, job_id: u64) {
        let script = format!(
            r#"
import _bundledb

def submit(details, job_data):
    try:
        return _bundledb.get_job_by_id({job_id})
    except Exception as e:
        return {{"error": str(e)}}
"#
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_bundle_db_delete_job(&self, hash: &str, job_json: &str) {
        let script = format!(
            r#"
import _bundledb
import json

def submit(details, job_data):
    job = json.loads('{job_json}')
    try:
        _bundledb.delete_job(job)
        return {{"error": False}}
    except Exception as e:
        return {{"error": str(e)}}
"#
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_file_list_no_job_working_directory(&self, hash: &str, working_dir: &str) {
        let script = format!(
            r#"
def working_directory(details, job_data):
    return "{working_dir}"
"#
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_job_status_complete(&self, hash: &str, status_json: &str) {
        let script = format!(
            r"
def status(details, job_data):
    import json
    return json.loads('{status_json}')
"
        );
        self.write_script(hash, &script, &[]);
    }

    pub fn write_job_cancel_check_status(
        &self,
        hash: &str,
        scheduler_id: u64,
        job_id: u64,
        cluster: &str,
        status_json: &str,
        cancel_result: &str,
    ) {
        let script = format!(
            r#"
def cancel(details, job_data):
    assert details["job_id"] == {job_id}
    assert details["cluster"] == "{cluster}"
    assert details["scheduler_id"] == {scheduler_id}
    return {cancel_result}

def status(details, job_data):
    assert details["job_id"] == {job_id}
    assert details["scheduler_id"] == {scheduler_id}
    assert details["cluster"] == "{cluster}"
    import json
    return json.loads('{status_json}')
"#
        );
        self.write_script(hash, &script, &[]);
    }
}
