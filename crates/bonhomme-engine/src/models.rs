use bonhomme_core::{
    Branch, MergeConflict, MergeOutcome, Operation, OperationRecord, RenderedFile, Repository,
    SemanticGraph,
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
    pub materialization: MaterializationEvidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedGraph {
    pub repository: Repository,
    pub branch: Branch,
    pub operations: Vec<OperationRecord>,
    pub graph: SemanticGraph,
    pub cache_status: CacheStatus,
    pub materialization: MaterializationEvidence,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CacheStatus {
    Hit,
    Incremental,
    Miss,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MaterializationEvidence {
    pub total_operations: i64,
    pub checkpoint_operations: i64,
    pub replayed_operations: i64,
    pub prefix_fingerprint_verified: bool,
    pub graph_invariants_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_rejection: Option<CheckpointRejection>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CheckpointRejection {
    AheadOfRequestedPosition,
    PrefixFingerprintMismatch,
    InvalidGraph,
    AppliedOperationsMismatch,
    UnsupportedProtocol,
    SnapshotContextMismatch,
    InvalidEncoding,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub outcome: MergeOutcome,
    pub conflicts: Vec<MergeConflict>,
    pub conflict_id: Option<Uuid>,
    pub source_branch: Branch,
    pub target_branch: Branch,
    pub appended_operations: Vec<OperationRecord>,
    pub target_position: i64,
    pub files: Vec<RenderedFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    SliceApply,
    Merge,
}

impl ConflictKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictKind::SliceApply => "slice_apply",
            ConflictKind::Merge => "merge",
        }
    }
}

impl std::str::FromStr for ConflictKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "slice_apply" => Ok(ConflictKind::SliceApply),
            "merge" => Ok(ConflictKind::Merge),
            other => anyhow::bail!("unknown conflict kind {other}"),
        }
    }
}

impl std::fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictState {
    Open,
    Resolving,
    Resolved,
    Abandoned,
}

impl ConflictState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictState::Open => "open",
            ConflictState::Resolving => "resolving",
            ConflictState::Resolved => "resolved",
            ConflictState::Abandoned => "abandoned",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, ConflictState::Open | ConflictState::Resolving)
    }
}

impl std::str::FromStr for ConflictState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "open" => Ok(ConflictState::Open),
            "resolving" => Ok(ConflictState::Resolving),
            "resolved" => Ok(ConflictState::Resolved),
            "abandoned" => Ok(ConflictState::Abandoned),
            other => anyhow::bail!("unknown conflict state {other}"),
        }
    }
}

impl std::fmt::Display for ConflictState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRecord {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub kind: ConflictKind,
    pub state: ConflictState,
    pub source_branch_id: Option<Uuid>,
    pub target_branch_id: Option<Uuid>,
    pub source_branch_name: String,
    pub target_branch_name: String,
    pub slice_id: Option<Uuid>,
    pub source_changeset_id: Option<Uuid>,
    pub base_position: i64,
    pub target_position: i64,
    pub conflicts: Vec<MergeConflict>,
    pub proposed_operations: Vec<Operation>,
    pub resolution_changeset_id: Option<Uuid>,
    pub acknowledged_drop: bool,
    pub created_by: String,
    pub resolved_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingConflict {
    pub repository_id: Uuid,
    pub kind: ConflictKind,
    pub source_branch_id: Uuid,
    pub target_branch_id: Uuid,
    pub source_branch_name: String,
    pub target_branch_name: String,
    pub slice_id: Option<Uuid>,
    pub source_changeset_id: Option<Uuid>,
    pub base_position: i64,
    pub target_position: i64,
    pub conflicts: Vec<MergeConflict>,
    pub proposed_operations: Vec<Operation>,
    pub created_by: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ConflictPositions {
    pub base: i64,
    pub target: i64,
}

impl ConflictPositions {
    pub fn new(base: i64, target: i64) -> Self {
        Self { base, target }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ApplySliceResult {
    Applied {
        changeset_id: Uuid,
        appended_operations: Vec<OperationRecord>,
    },
    Conflict {
        conflict: Box<ConflictRecord>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StillConflictingReason {
    ConflictNotOpen,
    MissingTargetBranch,
    RecoveryRejected,
    UnfaithfulRecovery,
    RequiresDropAcknowledgement,
    ValidationRejected,
    CompilerRejected,
    BranchAdvanced,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ResolveConflictResult {
    Resolved {
        conflict: Box<ConflictRecord>,
        changeset_id: Uuid,
        appended_operations: Vec<OperationRecord>,
        resumed_merge: Option<Box<MergeResult>>,
    },
    StillConflicting {
        conflict: Box<ConflictRecord>,
        reason: StillConflictingReason,
        conflicts: Vec<MergeConflict>,
        unfaithful_files: Vec<String>,
        diagnostic: String,
    },
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

/// Durable lifecycle state for an agent job.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentJobState {
    Queued,
    Leased,
    Succeeded,
    Failed,
    Cancelled,
}

impl AgentJobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

impl std::str::FromStr for AgentJobState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "leased" => Ok(Self::Leased),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => anyhow::bail!("unknown agent job state {other}"),
        }
    }
}

impl std::fmt::Display for AgentJobState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Backend-neutral enqueue request. `kind`, `payload`, and the eventual result are intentionally
/// opaque to the engine so workers can evolve independently of the durable queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJobSpec {
    pub tenant_key: String,
    pub kind: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub max_attempts: i64,
    pub retry_backoff_seconds: i64,
    pub max_backoff_seconds: i64,
    pub available_at: Option<DateTime<Utc>>,
}

impl AgentJobSpec {
    pub fn new(
        tenant_key: impl Into<String>,
        kind: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            tenant_key: tenant_key.into(),
            kind: kind.into(),
            idempotency_key: idempotency_key.into(),
            payload,
            max_attempts: 3,
            retry_backoff_seconds: 1,
            max_backoff_seconds: 300,
            available_at: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJob {
    pub id: Uuid,
    pub tenant_key: String,
    pub repository_id: Uuid,
    pub kind: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub state: AgentJobState,
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub retry_backoff_seconds: i64,
    pub max_backoff_seconds: i64,
    pub available_at: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// A lease receipt. The opaque token is returned only when a worker acquires a lease and is never
/// persisted in plaintext or included in ordinary job reads/listings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeasedAgentJob {
    pub job: AgentJob,
    pub lease_token: String,
}

impl AgentJob {
    pub(crate) fn retry_delay_seconds(&self) -> i64 {
        let exponent = self.attempt_count.saturating_sub(1).clamp(0, 62) as u32;
        let multiplier = 1_i64.checked_shl(exponent).unwrap_or(i64::MAX);
        self.retry_backoff_seconds
            .saturating_mul(multiplier)
            .min(self.max_backoff_seconds)
    }
}
