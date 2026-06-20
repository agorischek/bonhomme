use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::Operation;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub name: String,
    pub base_branch_id: Option<Uuid>,
    pub base_position: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub task_id: Uuid,
    pub branch_id: Uuid,
    pub title: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    pub changeset_id: Uuid,
    pub position: i64,
    pub operation: Operation,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SymbolNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub body: Option<String>,
    pub metadata: Value,
    pub ordinal: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceNode {
    pub id: Uuid,
    pub from_symbol_id: Uuid,
    pub to_symbol_id: Uuid,
    pub kind: String,
    pub ordinal: i64,
}
