from django.db import models


class Job(models.Model):
    # The job controller job ID
    job_id = models.BigIntegerField(blank=True, null=True, default=None, db_index=True)

    # The scheduler/bundle ID
    scheduler_id = models.BigIntegerField(blank=True, null=True, default=None, db_index=True)

    # If this job is waiting on a submit bundle function (Controller won't try to check job status or resubmit while
    # this flag is true)
    submitting = models.BooleanField(default=False)
    submitting_count = models.IntegerField(default=0)

    # The bundle hash of this job
    bundle_hash = models.CharField(max_length=40)

    # The working directory of this job
    working_directory = models.CharField(max_length=512)

    # If the job is queued and waiting for a bundle
    queued = models.BooleanField(default=False, db_index=True)

    # The job parameters if the job is to be queued
    params = models.TextField()

    # If the job is currently running
    running = models.BooleanField(default=True)


class JobStatusModel(models.Model):
    # The job this status object is for
    job = models.ForeignKey(Job, on_delete=models.CASCADE, related_name='status')

    # What this update was for, usually a job step id or system
    what = models.CharField(max_length=128)

    # The state for the update
    state = models.IntegerField(db_index=True)
