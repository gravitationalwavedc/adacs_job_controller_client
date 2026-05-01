use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Model {
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
