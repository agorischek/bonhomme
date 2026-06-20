use bonhomme_core::{Branch, ChangeSet, RenderedFile, Repository, SemanticGraph, Task};
use bonhomme_engine::MergeResult;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(super) struct DemoMethod {
    pub(super) name: String,
    pub(super) label: String,
    pub(super) body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnAgentsRequest {
    pub count: usize,
    pub include_conflicts: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoState {
    pub repository: Repository,
    pub main_branch: Branch,
    pub branches: Vec<BranchSummary>,
    pub tasks: Vec<Task>,
    pub changesets: Vec<ChangeSet>,
    pub operations: Vec<OperationView>,
    pub main_graph: SemanticGraph,
    pub rendered_files: Vec<RenderedFile>,
    pub metrics: DemoMetrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummary {
    pub id: Uuid,
    pub name: String,
    pub base_position: i64,
    pub status: BranchStatus,
    pub own_operation_count: usize,
    pub created_symbol_count: usize,
    pub created_method_names: Vec<String>,
    pub created_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BranchStatus {
    Main,
    Empty,
    Ready,
    Merged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    pub id: Uuid,
    pub branch_id: Uuid,
    pub branch_name: String,
    pub changeset_id: Uuid,
    pub position: i64,
    pub op_type: String,
    pub symbol_id: Option<Uuid>,
    pub symbol_name: Option<String>,
    pub symbol_kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoMetrics {
    pub branch_count: usize,
    pub agent_count: usize,
    pub merged_agent_count: usize,
    pub pending_agent_count: usize,
    pub operation_count: usize,
    pub symbol_count: usize,
    pub reference_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoMergeRun {
    pub results: Vec<MergeResult>,
    pub state: DemoState,
}
