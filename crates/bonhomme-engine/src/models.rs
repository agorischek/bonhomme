use bonhomme_core::{
    Branch, MergeConflict, MergeOutcome, OperationRecord, RenderedFile, Repository, SemanticGraph,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedBranch {
    pub repository: Repository,
    pub branch: Branch,
    pub operations: Vec<OperationRecord>,
    pub graph: SemanticGraph,
    pub files: Vec<RenderedFile>,
    pub cache_status: CacheStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedGraph {
    pub repository: Repository,
    pub branch: Branch,
    pub operations: Vec<OperationRecord>,
    pub graph: SemanticGraph,
    pub cache_status: CacheStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CacheStatus {
    Hit,
    Miss,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub outcome: MergeOutcome,
    pub conflicts: Vec<MergeConflict>,
    pub source_branch: Branch,
    pub target_branch: Branch,
    pub appended_operations: Vec<OperationRecord>,
    pub target_position: i64,
    pub files: Vec<RenderedFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub attachment_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSlice {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    pub base_position: i64,
    pub root_symbols: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFileSnapshot {
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    pub path: String,
    pub content_hash: String,
    pub byte_len: i64,
    pub handler: String,
    pub file_symbol_id: Option<Uuid>,
    pub last_import_position: i64,
    pub importer_version: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSourceFileSnapshot {
    pub path: String,
    pub content_hash: String,
    pub byte_len: i64,
    pub handler: String,
    pub file_symbol_id: Option<Uuid>,
    pub last_import_position: i64,
    pub importer_version: String,
}
