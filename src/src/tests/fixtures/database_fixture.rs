//! Database fixture for integration tests.
//!
//! Provides a real SQLite database with proper schema initialization.
//! Matches C++ DatabaseFixture functionality.

use crate::db;
use sea_orm::{ConnectionTrait, DbBackend};
use std::path::PathBuf;
use tempfile::TempDir;

pub struct DatabaseFixture {
    pub db_path: PathBuf,
    pub temp_dir: TempDir,
}

impl DatabaseFixture {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        Self { db_path, temp_dir }
    }

    pub async fn initialize(&self) {
        let db_url = format!("sqlite:{}?mode=rwc", self.db_path.display());
        db::initialize(&db_url)
            .await
            .expect("Failed to initialize database");

        let db = db::get_db();

        // Create schema
        let schema_job = r#"
            CREATE TABLE IF NOT EXISTS jobclient_job (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER,
                scheduler_id INTEGER,
                submitting BOOLEAN NOT NULL DEFAULT 0,
                submitting_count INTEGER NOT NULL DEFAULT 0,
                bundle_hash TEXT NOT NULL DEFAULT '',
                working_directory TEXT NOT NULL DEFAULT '',
                running BOOLEAN NOT NULL DEFAULT 0,
                deleted BOOLEAN NOT NULL DEFAULT 0,
                deleting BOOLEAN NOT NULL DEFAULT 0
            );
        "#;

        let schema_status = r#"
            CREATE TABLE IF NOT EXISTS jobclient_jobstatus (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                what TEXT NOT NULL DEFAULT '',
                state INTEGER NOT NULL DEFAULT 0,
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
    }

    pub fn get_db_path(&self) -> &PathBuf {
        &self.db_path
    }
}

impl Default for DatabaseFixture {
    fn default() -> Self {
        Self::new()
    }
}
