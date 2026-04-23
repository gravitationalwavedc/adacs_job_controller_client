pub mod job;
pub mod jobstatus;

use crate::db::job::Column as JobColumn;
use crate::db::job::Entity as Job;
use crate::db::jobstatus::Column as JobStatusColumn;
use crate::db::jobstatus::Entity as JobStatus;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, Set,
};

use std::sync::atomic::{AtomicPtr, Ordering};

static DB_PTR: AtomicPtr<DatabaseConnection> = AtomicPtr::new(std::ptr::null_mut());

pub async fn initialize(url: &str) -> Result<(), sea_orm::DbErr> {
    let db = sea_orm::Database::connect(url).await?;
    let boxed = Box::new(db);
    let ptr = Box::into_raw(boxed);

    let old_ptr = DB_PTR.swap(ptr, Ordering::SeqCst);
    if !old_ptr.is_null() {
        // Clean up the old connection to prevent leaks
        unsafe {
            drop(Box::from_raw(old_ptr));
        }
    }
    Ok(())
}

pub fn get_db() -> &'static DatabaseConnection {
    let ptr = DB_PTR.load(Ordering::SeqCst);
    if ptr.is_null() {
        panic!("DB not initialized");
    }
    unsafe { &*ptr }
}

#[cfg(test)]
pub async fn reset_for_test(url: &str) -> Result<(), sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, DbBackend};

    // Drop the old connection if it exists
    let old_ptr = DB_PTR.load(Ordering::SeqCst);
    if !old_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(old_ptr));
        }
    }

    // Create a fresh connection
    let db = sea_orm::Database::connect(url).await?;

    // Create schema
    let schema_job = r#"
        CREATE TABLE IF NOT EXISTS jobclient_job (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER,
            scheduler_id INTEGER,
            submitting BOOLEAN NOT NULL,
            submitting_count INTEGER NOT NULL,
            bundle_hash TEXT NOT NULL,
            working_directory TEXT NOT NULL,
            running BOOLEAN NOT NULL,
            deleted BOOLEAN NOT NULL,
            deleting BOOLEAN NOT NULL
        );
    "#;
    let schema_status = r#"
        CREATE TABLE IF NOT EXISTS jobclient_jobstatus (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            what TEXT NOT NULL,
            state INTEGER NOT NULL,
            job_id INTEGER NOT NULL,
            FOREIGN KEY(job_id) REFERENCES jobclient_job(id)
        );
    "#;
    let _ = db
        .execute(sea_orm::Statement::from_string(
            DbBackend::Sqlite,
            schema_job.to_string(),
        ))
        .await;
    let _ = db
        .execute(sea_orm::Statement::from_string(
            DbBackend::Sqlite,
            schema_status.to_string(),
        ))
        .await;

    // Store in the global pointer
    let boxed = Box::new(db);
    let ptr = Box::into_raw(boxed);
    let _ = DB_PTR.swap(ptr, Ordering::SeqCst);
    Ok(())
}

pub async fn get_running_jobs(db: &DatabaseConnection) -> Result<Vec<job::Model>, sea_orm::DbErr> {
    Job::find()
        .filter(JobColumn::Running.eq(true))
        .all(db)
        .await
}

pub async fn get_job_by_id(
    db: &DatabaseConnection,
    id: i64,
) -> Result<Option<job::Model>, sea_orm::DbErr> {
    Job::find_by_id(id).one(db).await
}

pub async fn get_job_by_job_id(
    db: &DatabaseConnection,
    job_id_val: i64,
) -> Result<Option<job::Model>, sea_orm::DbErr> {
    Job::find()
        .filter(JobColumn::JobId.eq(job_id_val))
        .one(db)
        .await
}

pub async fn delete_job(
    db: &DatabaseConnection,
    id: i64,
) -> Result<sea_orm::DeleteResult, sea_orm::DbErr> {
    Job::delete_by_id(id).exec(db).await
}

pub async fn get_or_create_by_job_id(
    db: &DatabaseConnection,
    job_id_val: i64,
) -> Result<job::Model, sea_orm::DbErr> {
    let existing = Job::find()
        .filter(JobColumn::JobId.eq(job_id_val))
        .one(db)
        .await?;

    if let Some(job) = existing {
        Ok(job)
    } else {
        Ok(job::Model {
            id: 0,
            job_id: None,
            scheduler_id: None,
            submitting: false,
            submitting_count: 0,
            bundle_hash: "".to_string(),
            working_directory: "".to_string(),
            running: false,
            deleted: false,
            deleting: false,
        })
    }
}

pub async fn get_job_status_by_job_id_and_what(
    db: &DatabaseConnection,
    job_id: i64,
    what: &str,
) -> Result<Vec<jobstatus::Model>, sea_orm::DbErr> {
    JobStatus::find()
        .filter(JobStatusColumn::JobId.eq(job_id))
        .filter(JobStatusColumn::What.eq(what))
        .all(db)
        .await
}

pub async fn get_job_status_by_job_id(
    db: &DatabaseConnection,
    job_id: i64,
) -> Result<Vec<jobstatus::Model>, sea_orm::DbErr> {
    JobStatus::find()
        .filter(JobStatusColumn::JobId.eq(job_id))
        .all(db)
        .await
}

pub async fn delete_status_by_id_list(
    db: &DatabaseConnection,
    ids: Vec<i64>,
) -> Result<sea_orm::DeleteResult, sea_orm::DbErr> {
    JobStatus::delete_many()
        .filter(JobStatusColumn::Id.is_in(ids))
        .exec(db)
        .await
}

pub async fn save_job(
    db: &DatabaseConnection,
    job: job::Model,
) -> Result<job::Model, sea_orm::DbErr> {
    if job.id == 0 {
        let mut active = job.into_active_model();
        active.id = sea_orm::ActiveValue::NotSet;
        active.insert(db).await
    } else {
        let mut active = job::ActiveModel::new();
        active.id = Set(job.id);
        active.job_id = Set(job.job_id);
        active.scheduler_id = Set(job.scheduler_id);
        active.submitting = Set(job.submitting);
        active.submitting_count = Set(job.submitting_count);
        active.bundle_hash = Set(job.bundle_hash);
        active.working_directory = Set(job.working_directory);
        active.running = Set(job.running);
        active.deleted = Set(job.deleted);
        active.deleting = Set(job.deleting);
        active.update(db).await
    }
}

pub async fn save_status(
    db: &DatabaseConnection,
    status: jobstatus::Model,
) -> Result<jobstatus::Model, sea_orm::DbErr> {
    if status.id == 0 {
        let mut active = status.into_active_model();
        active.id = sea_orm::ActiveValue::NotSet;
        active.insert(db).await
    } else {
        let mut active = jobstatus::ActiveModel::new();
        active.id = Set(status.id);
        active.what = Set(status.what);
        active.state = Set(status.state);
        active.job_id = Set(status.job_id);
        active.update(db).await
    }
}
