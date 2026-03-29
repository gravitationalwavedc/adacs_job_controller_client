# C++ Test Cases Checklist

## Files Module

### test_file_download.cpp
- [ ] **test_get_file_download_job_not_exist**
  - **Setup**: Websocket server and database initialized; no job with ID 1234 + 1000 exists.
  - **Act**: Send `FILE_DOWNLOAD` message for non-existent job.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "Job does not exist".
  - **Why**: Verifies error handling for invalid job IDs.
- [ ] **test_get_file_download_job_submitting**
  - **Setup**: Job in database marked as `submitting = 1`.
  - **Act**: Send `FILE_DOWNLOAD` message.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "Job is not submitted".
  - **Why**: Ensures downloads are blocked while a job is still in the submission process.
- [ ] **test_get_file_download_job_outside_working_directory**
  - **Setup**: Job working directory set to `/usr`.
  - **Act**: Send `FILE_DOWNLOAD` message with path `../` + tempFile.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "outside the working directory".
  - **Why**: Security check to prevent path traversal attacks.
- [ ] **test_get_file_download_job_file_not_exist**
  - **Setup**: Valid job in database.
  - **Act**: Send `FILE_DOWNLOAD` for a file path that does not exist.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "does not exist".
  - **Why**: Verifies handling of requests for non-existent files.
- [ ] **test_get_file_download_job_file_is_a_directory**
  - **Setup**: Valid job; target path is a directory.
  - **Act**: Send `FILE_DOWNLOAD`.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "is not a file".
  - **Why**: Prevents attempting to download a directory as a single file.
- [ ] **test_get_file_download_no_job_outside_working_directory**
  - **Setup**: Bundle-based file listing with working directory `/usr`.
  - **Act**: Send `FILE_DOWNLOAD` with job ID 0 and path `../` + tempFile.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "outside the working directory".
  - **Why**: Security check for bundle-based downloads (no specific job).
- [ ] **test_get_file_download_no_job_directory_not_exist**
  - **Setup**: Bundle-based file listing.
  - **Act**: Send `FILE_DOWNLOAD` for non-existent file.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "does not exist".
  - **Why**: Handles missing files in bundle-based download requests.
- [ ] **test_get_file_download_no_job_file_is_a_directory**
  - **Setup**: Bundle-based file listing; target is a directory.
  - **Act**: Send `FILE_DOWNLOAD`.
  - **Assert**: Receive `FILE_DOWNLOAD_ERROR` with "is not a file".
  - **Why**: Integrity check for bundle-based downloads.
- [ ] **test_get_file_download_job_success**
  - **Setup**: Valid job; random data written to a temp file.
  - **Act**: Send `FILE_DOWNLOAD`.
  - **Assert**: Receive `FILE_DOWNLOAD_DETAILS` followed by file chunks; content must match original.
  - **Why**: Verifies the end-to-end successful download of a file associated with a job.
- [ ] **test_get_file_download_no_job_success**
  - **Setup**: Bundle-based listing; random data written.
  - **Act**: Send `FILE_DOWNLOAD` with job ID 0.
  - **Assert**: Receive `FILE_DOWNLOAD_DETAILS` and matching data chunks.
  - **Why**: Verifies the end-to-end successful download of a file not associated with a specific job.

### test_file_list.cpp
- [ ] **test_get_file_list_job_not_exist**
  - **Setup**: Database with no job for ID 2234.
  - **Act**: Send `FILE_LIST`.
  - **Assert**: Receive `FILE_LIST_ERROR` "Job does not exist".
  - **Why**: Error handling for invalid job IDs during file listing.
- [ ] **test_get_file_list_job_submitting**
  - **Setup**: Job marked as `submitting = 1`.
  - **Act**: Send `FILE_LIST`.
  - **Assert**: Receive `FILE_LIST_ERROR` "Job is not submitted".
  - **Why**: Prevents listing files while a job is being submitted.
- [ ] **test_get_file_list_job_outside_working_directory**
  - **Setup**: Job directory set to `/usr`.
  - **Act**: Send `FILE_LIST` for `../` path.
  - **Assert**: Receive `FILE_LIST_ERROR` "outside the working directory".
  - **Why**: Path traversal security check for file listing.
- [ ] **test_get_file_list_job_directory_not_exist**
  - **Setup**: Valid job.
  - **Act**: Send `FILE_LIST` for non-existent directory.
  - **Assert**: Receive `FILE_LIST_ERROR` "does not exist".
  - **Why**: Handles requests for missing directories.
- [ ] **test_get_file_list_job_directory_is_a_file**
  - **Setup**: Target path is a file, not a directory.
  - **Act**: Send `FILE_LIST`.
  - **Assert**: Receive `FILE_LIST_ERROR` "is not a directory".
  - **Why**: Ensures listing is only attempted on directories.
- [ ] **test_get_file_list_job_success_recursive**
  - **Setup**: Valid job with nested directories and files.
  - **Act**: Send `FILE_LIST` with recursion enabled.
  - **Assert**: Receive `FILE_LIST` with correct count and details for all items.
  - **Why**: Verifies recursive directory listing for a job.
- [ ] **test_get_file_list_job_success_not_recursive**
  - **Setup**: Valid job with nested items.
  - **Act**: Send `FILE_LIST` with recursion disabled.
  - **Assert**: Receive `FILE_LIST` with only top-level items.
  - **Why**: Verifies non-recursive directory listing for a job.
- [ ] **test_get_file_list_no_job_success**
  - **Setup**: Bundle-based listing.
  - **Act**: Send `FILE_LIST` with job ID 0.
  - **Assert**: Receive `FILE_LIST` with correct item metadata.
  - **Why**: Verifies bundle-based directory listing.

### test_file_upload.cpp
- [ ] **test_file_upload_job_based_success**
  - **Setup**: Valid job and websocket server.
  - **Act**: Trigger `UPLOAD_FILE` message handler.
  - **Assert**: File created in target directory with matching content.
  - **Why**: Verifies successful job-associated file upload.
- [ ] **test_file_upload_bundle_based_success**
  - **Setup**: Bundle-based working directory config.
  - **Act**: Trigger `UPLOAD_FILE` with job ID 0.
  - **Assert**: File created in bundle's directory.
  - **Why**: Verifies bundle-based file upload.
- [ ] **test_file_upload_invalid_path_outside_working_directory**
  - **Setup**: Valid job.
  - **Act**: Attempt upload to `../outside_file.txt`.
  - **Assert**: Receive `FILE_UPLOAD_ERROR` "outside the working directory".
  - **Why**: Security check against path traversal during upload.
- [ ] **test_file_upload_invalid_job_id**
  - **Setup**: No job for given ID.
  - **Act**: Attempt upload.
  - **Assert**: Receive `FILE_UPLOAD_ERROR` "Job does not exist".
  - **Why**: Ensures job existence before allowing uploads.
- [ ] **test_file_upload_job_submitting**
  - **Setup**: Job state is `submitting`.
  - **Act**: Attempt upload.
  - **Assert**: Receive `FILE_UPLOAD_ERROR` "Job is not submitted".
  - **Why**: Prevents uploads for jobs currently in submission.
- [ ] **test_file_upload_large_file**
  - **Setup**: Valid job.
  - **Act**: Upload 1MB of binary data in chunks.
  - **Assert**: File exists and matches source data perfectly.
  - **Why**: Verifies chunked handling of larger binary files.
- [ ] **test_file_upload_zero_byte_file**
  - **Setup**: Valid job.
  - **Act**: Upload 0-byte file.
  - **Assert**: File created with size 0.
  - **Why**: Ensures empty files are handled correctly.
- [ ] **test_file_upload_file_size_mismatch**
  - **Setup**: Valid job.
  - **Act**: Declare 1000 bytes but send less.
  - **Assert**: Receive `FILE_UPLOAD_ERROR` "File size mismatch".
  - **Why**: Integrity check to ensure complete file delivery.
- [ ] **test_file_upload_nested_directory_creation**
  - **Setup**: Valid job.
  - **Act**: Upload to `subdir/nested/file.txt`.
  - **Assert**: `subdir/nested/` directories created; file exists.
  - **Why**: Verifies automatic directory creation during upload.
- [ ] **test_file_upload_actual_bigger_than_declared**
  - **Setup**: Valid job.
  - **Act**: Declare 128 bytes but send 256.
  - **Assert**: Receive `FILE_UPLOAD_ERROR` "File size mismatch".
  - **Why**: Integrity check against over-sending data.
- [ ] **test_multiple_concurrent_file_uploads**
  - **Setup**: Five concurrent upload requests.
  - **Act**: Initiate all uploads simultaneously.
  - **Assert**: All five files created correctly without corruption.
  - **Why**: Verifies thread safety and concurrency of the upload system.

## Jobs Module

### test_archive_job.cpp
- [ ] **test_archive_success**
  - **Setup**: Job with valid working directory and files.
  - **Act**: Call `archiveJob(job)`.
  - **Assert**: Returns true; `archive.tar.gz` created in working directory.
  - **Why**: Verifies the job directory archiving functionality.
- [ ] **test_archive_fail**
  - **Setup**: Job with root directory (restricted).
  - **Act**: Call `archiveJob(job)`.
  - **Assert**: Returns false; no archive created.
  - **Why**: Verifies error handling for failed archiving.

### test_cancel_job.cpp
- [ ] **test_cancel_job_job_not_exists**
  - **Setup**: Database with no matching job ID.
  - **Act**: Send `CANCEL_JOB`.
  - **Assert**: UPDATE_JOB message sent with `CANCELLED` status.
  - **Why**: Gracefully handles cancellation for missing jobs.
- [ ] **test_cancel_job_job_not_running**
  - **Setup**: Job exists but `running = false`.
  - **Act**: Send `CANCEL_JOB`.
  - **Assert**: UPDATE_JOB message sent with `CANCELLED` status.
  - **Why**: Gracefully handles cancellation for already stopped jobs.
- [ ] **test_cancel_job_job_submitting**
  - **Setup**: Job marked as `submitting = true`.
  - **Act**: Send `CANCEL_JOB`.
  - **Assert**: Job remains running; no server notification sent.
  - **Why**: Prevents cancellation while a job is in the middle of submission.
- [ ] **test_cancel_job_not_running_after_status_check**
  - **Setup**: Job initially running, but status check shows completion.
  - **Act**: Send `CANCEL_JOB`.
  - **Assert**: Job marked `COMPLETED`; archive created; server notified.
  - **Why**: Correctly updates state if job finishes just before cancellation.
- [ ] **test_cancel_job_running_after_status_check_cancel_success_not_already_cancelled**
  - **Setup**: Job running; bundle-side cancellation succeeds.
  - **Act**: Send `CANCEL_JOB`.
  - **Assert**: Job marked `CANCELLED`; archive created.
  - **Why**: Verifies successful cancellation through the bundle interface.

### test_check_status_many_bundles.cpp
- [ ] **test_check_all_job_status**
  - **Setup**: 100 jobs initialized across unique bundles.
  - **Act**: Call `checkAllJobsStatus()`.
  - **Assert**: All 100 jobs finished/archived; 100 notifications sent.
  - **Why**: Stress test for status checking and cleanup across many jobs.

### test_check_status.cpp
- [ ] **test_check_status_no_status_complete**
  - **Setup**: Job running; status check returns `complete: true`.
  - **Act**: `checkJobStatusImpl`.
  - **Assert**: Job marked `COMPLETED`; archive created; server notified.
  - **Why**: Verifies job cleanup when it finishes naturally.
- [ ] **test_check_status_job_running_new_status**
  - **Setup**: Job running; status check returns new info.
  - **Act**: `checkJobStatusImpl`.
  - **Assert**: Status record created in DB; server notified.
  - **Why**: Verifies status updates are propagated to the server.
- [ ] **test_check_status_job_running_error_1**
  - **Setup**: Status check returns `ERROR`.
  - **Act**: `checkJobStatusImpl`.
  - **Assert**: Job stopped; archived; server notified of error.
  - **Why**: Verifies handling of job-level errors reported by scheduler.

### test_delete_job.cpp
- [ ] **test_delete_job_success**
  - **Setup**: Job exists; bundle-side deletion succeeds.
  - **Act**: Send `DELETE_JOB`.
  - **Assert**: Job marked `deleted = true` in DB; server notified.
  - **Why**: Verifies successful job deletion.

### test_job_submit.cpp
- [ ] **test_submit_timeout**
  - **Setup**: Job marked `submitting = true`.
  - **Act**: Repeatedly send `SUBMIT_JOB`.
  - **Assert**: `submittingCount` increments; eventually resubmits.
  - **Why**: Verifies retry/timeout logic for stalled submissions.
- [ ] **test_submit_error_zero**
  - **Setup**: Bundle returns error (0).
  - **Act**: Send `SUBMIT_JOB`.
  - **Assert**: Job state set to `ERROR`; server notified.
  - **Why**: Handles bundle-side submission failures.

## Bundle Module

### bundle_db_tests.cpp
- [ ] **test_create_or_update_job**
  - **Setup**: Bundle manager and websocket mock.
  - **Act**: Call `runBundle_json` with "submit" command.
  - **Assert**: Result matches JSON sent via websocket response.
  - **Why**: Tests bundle interaction with database via websocket proxy.
- [ ] **test_delete_job**
  - **Setup**: Valid job JSON.
  - **Act**: Call `runBundle_json`.
  - **Assert**: Correct job ID sent for deletion in mock server.
  - **Why**: Verifies deletion requests from bundles are proxied correctly.

### bundle_logging_tests.cpp
- [ ] **test_simple_stdout**
  - **Setup**: Bundle logging fixture.
  - **Act**: Bundle writes to stdout.
  - **Assert**: Log captured with "Bundle [hash]: ..." prefix.
  - **Why**: Verifies standard output capture from bundles.

## Websocket Module

### ping_pong_tests.cpp
- [ ] **test_checkPings_send_ping_success**
  - **Setup**: Active websocket connection.
  - **Act**: Call `checkPings()`.
  - **Assert**: Ping sent; Pong received; timestamps updated.
  - **Why**: Verifies connection keep-alive logic.
- [ ] **test_checkPings_handle_zero_time**
  - **Setup**: Active connection; mock pong timeout.
  - **Act**: Call `checkPings()`.
  - **Assert**: `std::runtime_error` thrown; application aborted.
  - **Why**: Verifies disconnection on unresponsive server.

### WebsocketInterfaceTests.cpp
- [ ] **test_queueMessage**
  - **Setup**: Multiple messages with different priorities.
  - **Act**: Queue messages via `queueMessage()`.
  - **Assert**: Messages stored in correct priority buckets; ordering preserved.
  - **Why**: Verifies prioritized message queuing logic.
- [ ] **test_run**
  - **Setup**: Connection established; multiple messages queued.
  - **Act**: Call `run()` loop.
  - **Assert**: Messages sent in priority/source order.
  - **Why**: Verifies the main prioritized transmission loop.
