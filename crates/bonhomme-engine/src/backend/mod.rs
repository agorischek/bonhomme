//! The storage backend abstraction. `Storage` keeps the cross-cutting logic (branch op
//! collection, materialization, merge orchestration) and delegates the leaf persistence
//! operations to a `StorageBackend`, so the engine can target either Postgres (the hosted
//! server) or an embedded Turso / libSQL database (local coauth sessions, edge replicas)
//! without the rest of the system knowing.

mod postgres;
mod turso;

use anyhow::{Result, bail};
use bonhomme_core::{Branch, ChangeSet, OperationRecord, Repository, Task};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) use self::postgres::PostgresBackend;
use crate::models::{AgentJob, AgentJobSpec, PendingConflict};
use crate::{
    Attachment, ConflictRecord, ConflictState, PendingSourceFileSnapshot, SourceFileSnapshot,
    StoredSlice,
};
// `self::` disambiguates the child module from the extern `turso` crate.
pub(crate) use self::turso::TursoBackend;

pub(crate) struct PendingOperation {
    pub(crate) op_type: String,
    pub(crate) payload: Value,
}

pub(crate) struct PendingAttachment {
    pub(crate) attachment_type: String,
    pub(crate) payload: Value,
}

pub(crate) struct PendingOperationCommit {
    pub(crate) repository_id: Uuid,
    pub(crate) branch_id: Uuid,
    pub(crate) expected_current_position: i64,
    pub(crate) task_title: String,
    pub(crate) changeset_title: String,
    pub(crate) created_by: String,
    pub(crate) operations: Vec<PendingOperation>,
}

pub(crate) struct PendingConflictResolutionCommit {
    pub(crate) commit: PendingOperationCommit,
    pub(crate) conflict_id: Uuid,
    pub(crate) attachment: PendingAttachment,
    pub(crate) resolved_by: String,
    pub(crate) acknowledged_drop: bool,
}

pub(crate) struct CommittedOperations {
    pub(crate) operations: Vec<OperationRecord>,
}

pub(crate) struct CommittedConflictResolution {
    pub(crate) conflict: ConflictRecord,
    pub(crate) changeset: ChangeSet,
    pub(crate) operations: Vec<OperationRecord>,
}

pub(crate) struct LeaseAgentJobRequest<'a> {
    pub(crate) tenant_key: &'a str,
    pub(crate) repository_id: Uuid,
    pub(crate) owner: &'a str,
    pub(crate) lease_token_digest: &'a str,
    pub(crate) kind: Option<&'a str>,
    pub(crate) now: DateTime<Utc>,
    pub(crate) lease_expires_at: DateTime<Utc>,
}

pub(crate) struct AgentJobLease<'a> {
    pub(crate) tenant_key: &'a str,
    pub(crate) repository_id: Uuid,
    pub(crate) job_id: Uuid,
    pub(crate) owner: &'a str,
    pub(crate) lease_token_digest: &'a str,
    pub(crate) now: DateTime<Utc>,
}

#[derive(Debug)]
pub(crate) struct BranchAdvanced {
    pub(crate) branch_id: Uuid,
    pub(crate) expected: i64,
    pub(crate) actual: i64,
}

impl std::fmt::Display for BranchAdvanced {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "branch {} advanced from position {} to {}; retry merge",
            self.branch_id, self.expected, self.actual
        )
    }
}

impl std::error::Error for BranchAdvanced {}

#[derive(Clone, Debug)]
pub(crate) struct StoredGraphCheckpoint {
    pub(crate) operation_count: i64,
    pub(crate) operation_fingerprint: String,
    pub(crate) graph: Value,
}

/// The leaf persistence operations. Each is implemented per database; the high-level engine logic
/// lives on [`crate::Storage`] and is written once on top of this trait.
#[async_trait::async_trait]
pub(crate) trait StorageBackend: Send + Sync {
    async fn run_migrations(&self) -> Result<()>;

    async fn reset_repository_by_name(&self, name: &str) -> Result<(Repository, Branch)>;
    async fn create_repository(&self, name: &str) -> Result<Repository>;
    async fn repository_by_name(&self, name: &str) -> Result<Repository>;
    async fn repository_by_id(&self, id: Uuid) -> Result<Repository>;

    async fn ensure_main_branch(&self, repository_id: Uuid) -> Result<Branch>;
    /// Insert a child branch; returns `None` if a branch with that name already exists (its fork
    /// point is immutable, so the caller re-fetches the existing one).
    async fn insert_branch(
        &self,
        repository_id: Uuid,
        name: &str,
        base_branch_id: Uuid,
        base_position: i64,
    ) -> Result<Option<Branch>>;
    async fn delete_branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<()>;
    async fn branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<Branch>;
    async fn branch_by_id(&self, branch_id: Uuid) -> Result<Branch>;
    async fn list_branches(&self, repository_id: Uuid) -> Result<Vec<Branch>>;

    async fn create_task(&self, repository_id: Uuid, title: &str) -> Result<Task>;
    async fn list_tasks(&self, repository_id: Uuid) -> Result<Vec<Task>>;

    async fn create_changeset(
        &self,
        repository_id: Uuid,
        task_id: Uuid,
        branch_id: Uuid,
        title: &str,
        created_by: &str,
    ) -> Result<ChangeSet>;
    async fn list_changesets(&self, repository_id: Uuid) -> Result<Vec<ChangeSet>>;

    /// Allocate the next per-branch position and append the operation atomically.
    async fn append_operation(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        op_type: &str,
        payload: Value,
    ) -> Result<OperationRecord>;
    async fn append_operations(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        operations: Vec<PendingOperation>,
    ) -> Result<Vec<OperationRecord>> {
        let mut appended = Vec::with_capacity(operations.len());
        for operation in operations {
            appended.push(
                self.append_operation(
                    repository_id,
                    branch_id,
                    changeset_id,
                    &operation.op_type,
                    operation.payload,
                )
                .await?,
            );
        }
        Ok(appended)
    }
    async fn append_operations_if_branch_position(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        expected_current_position: i64,
        operations: Vec<PendingOperation>,
    ) -> Result<Vec<OperationRecord>>;
    async fn list_operations(&self, repository_id: Uuid) -> Result<Vec<OperationRecord>>;
    async fn list_own_operations(
        &self,
        branch_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<OperationRecord>>;

    async fn insert_slice(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        base_position: i64,
        root_symbols: Value,
    ) -> Result<StoredSlice>;
    async fn slice_by_id(&self, slice_id: Uuid) -> Result<StoredSlice>;

    async fn insert_conflict_with_attachment(
        &self,
        conflict: PendingConflict,
        attachment: PendingAttachment,
    ) -> Result<ConflictRecord> {
        let _ = (conflict, attachment);
        bail!("atomic conflict creation is not implemented for this backend")
    }
    async fn list_conflicts(&self, repository_id: Uuid) -> Result<Vec<ConflictRecord>>;
    async fn conflict_by_id(&self, conflict_id: Uuid) -> Result<ConflictRecord>;
    async fn update_conflict_state(
        &self,
        conflict_id: Uuid,
        state: ConflictState,
    ) -> Result<ConflictRecord>;
    async fn commit_operations(
        &self,
        commit: PendingOperationCommit,
    ) -> Result<CommittedOperations> {
        let _ = commit;
        bail!("atomic operation commit is not implemented for this backend")
    }
    async fn commit_conflict_resolution(
        &self,
        resolution: PendingConflictResolutionCommit,
    ) -> Result<CommittedConflictResolution> {
        let _ = resolution;
        bail!("atomic conflict resolution is not implemented for this backend")
    }

    async fn enqueue_agent_job(
        &self,
        repository_id: Uuid,
        spec: AgentJobSpec,
        request_fingerprint: String,
        now: DateTime<Utc>,
    ) -> Result<AgentJob>;
    async fn agent_job_by_id(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
    ) -> Result<AgentJob>;
    async fn list_agent_jobs(&self, tenant_key: &str, repository_id: Uuid)
    -> Result<Vec<AgentJob>>;
    async fn lease_agent_job(&self, request: LeaseAgentJobRequest<'_>) -> Result<Option<AgentJob>>;
    async fn heartbeat_agent_job(
        &self,
        lease: AgentJobLease<'_>,
        lease_expires_at: DateTime<Utc>,
    ) -> Result<AgentJob>;
    async fn succeed_agent_job(&self, lease: AgentJobLease<'_>, result: Value) -> Result<AgentJob>;
    async fn fail_agent_job(&self, lease: AgentJobLease<'_>, error: &str) -> Result<AgentJob>;
    async fn cancel_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AgentJob>;

    async fn add_attachment(
        &self,
        repository_id: Uuid,
        entity_type: &str,
        entity_id: Uuid,
        attachment_type: &str,
        payload: Value,
    ) -> Result<Attachment>;

    /// Most recent graph checkpoint for a branch. The caller must verify that its fingerprint
    /// matches the requested operation prefix before using it; backend rows are never trusted as
    /// replay authority.
    async fn latest_graph_checkpoint(
        &self,
        branch_id: Uuid,
    ) -> Result<Option<StoredGraphCheckpoint>>;
    async fn store_graph_cache(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        operation_count: i64,
        fingerprint: &str,
        graph: Value,
        rendered_files: Value,
    ) -> Result<()>;

    async fn list_source_file_snapshots(&self, branch_id: Uuid) -> Result<Vec<SourceFileSnapshot>>;
    async fn replace_source_file_snapshots(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        snapshots: Vec<PendingSourceFileSnapshot>,
    ) -> Result<()>;
}

/// Pick a backend from the connection URL: `postgres://…` → Postgres; `turso:`/`sqlite:`/`file:`
/// prefixes or `:memory:` → an embedded Turso database.
pub(crate) async fn connect(url: &str) -> Result<Arc<dyn StorageBackend>> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        return Ok(Arc::new(PostgresBackend::connect(url).await?));
    }
    if url == ":memory:" {
        return Ok(Arc::new(TursoBackend::connect(":memory:").await?));
    }
    if let Some(path) = url
        .strip_prefix("turso:")
        .or_else(|| url.strip_prefix("sqlite:"))
        .or_else(|| url.strip_prefix("file:"))
    {
        return Ok(Arc::new(TursoBackend::connect(path).await?));
    }
    bail!(
        "unsupported storage URL '{url}': expected postgres://…, turso:PATH, sqlite:PATH, file:PATH, or :memory:"
    )
}
