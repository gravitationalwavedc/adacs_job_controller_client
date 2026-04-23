use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, Default)]
#[sea_orm(table_name = "jobclient_job")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub job_id: Option<i64>,
    pub scheduler_id: Option<i64>,
    pub submitting: bool,
    pub submitting_count: i32,
    pub bundle_hash: String,
    pub working_directory: String,
    pub running: bool,
    pub deleted: bool,
    pub deleting: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::jobstatus::Entity")]
    JobStatus,
}

impl Related<super::jobstatus::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::JobStatus.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
