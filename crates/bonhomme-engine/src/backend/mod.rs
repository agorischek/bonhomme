//! The storage backend abstraction. `Storage` keeps the cross-cutting logic (branch op
//! collection, materialization, merge orchestration) and delegates the leaf persistence
//! operations to a `StorageBackend`, so the engine can target either Postgres (the hosted
//! server) or an embedded Turso / libSQL database (local coauth sessions, edge replicas)
//! without the rest of the system knowing.

mod postgres;
mod turso;

use anyhow::{Result, bail};
use bonhomme_core::{Branch, ChangeSet, OperationRecord, Repository, Task};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) use self::postgres::PostgresBackend;
use crate::{Attachment, PendingSourceFileSnapshot, SourceFileSnapshot, StoredSlice};
// `self::` disambiguates the child module from the extern `turso` crate.
pub(crate) use self::turso::TursoBackend;

pub(crate) struct PendingOperation {
    pub(crate) op_type: String,
    pub(crate) payload: Value,
}

/// The leaf persistence operations. Each is implemented per database; the high-level engine logic
/// lives on [`crate::Storage`] and is written once on top of this trait.
#[async_trait::async_trait]
pub(crate) trait StorageBackend: Send + Sync {
    async fn run_migrations(&self) -> Result<()>;

    async fn delete_repository_by_name(&self, name: &str) -> Result<()>;
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

    async fn add_attachment(
        &self,
        repository_id: Uuid,
        entity_type: &str,
        entity_id: Uuid,
        attachment_type: &str,
        payload: Value,
    ) -> Result<Attachment>;

    /// Cached `(graph, rendered_files)` JSON for a branch at a given fingerprint, if present.
    async fn get_graph_cache(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        fingerprint: &str,
    ) -> Result<Option<(Value, Value)>>;
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
