use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: i64,
    pub what: String,
    pub state: i32,
    pub job_id: i64,
}
