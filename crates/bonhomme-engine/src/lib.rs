mod backend;
mod materialization;
mod merge;
mod models;
mod rows;

#[cfg(test)]
mod turso_tests;

use anyhow::Result;
use bonhomme_core::{
    Branch, ChangeSet, LanguagePlugin, Operation, OperationRecord, Repository, Task,
};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use backend::StorageBackend;
pub use models::{
    Attachment, CacheStatus, MaterializedBranch, MaterializedGraph, MergeResult,
    PendingSourceFileSnapshot, SourceFileSnapshot, StoredSlice,
};

pub const DEFAULT_DATABASE_URL: &str = "postgres://bonhomme:bonhomme@localhost:54329/bonhomme";

/// The storage / merge engine. It is language-agnostic: every render, validate, import, and diff
/// goes through the injected [`LanguagePlugin`], so this module never depends on `ts`. It is also
/// database-agnostic: every persistence operation goes through a [`StorageBackend`], so it runs
/// on Postgres (the hosted server) or an embedded Turso database (local sessions) interchangeably.
#[derive(Clone)]
pub struct Storage {
    pub(crate) backend: Arc<dyn StorageBackend>,
    pub(crate) plugin: Arc<dyn LanguagePlugin>,
}

impl Storage {
    /// Connect using a backend chosen from the URL scheme: `postgres://…` → Postgres;
    /// `turso:`/`sqlite:`/`file:` prefixes or `:memory:` → an embedded Turso database.
    pub async fn connect(database_url: &str, plugin: Arc<dyn LanguagePlugin>) -> Result<Self> {
        let backend = backend::connect(database_url).await?;
        Ok(Self { backend, plugin })
    }

    pub async fn migrate(&self) -> Result<()> {
        self.backend.run_migrations().await
    }

    /// The configured language backend. The CLI/simulation layers render, import, diff, and
    /// validate through this rather than calling a concrete language module directly.
    pub fn plugin(&self) -> &dyn LanguagePlugin {
        self.plugin.as_ref()
    }

    pub async fn init_repository(&self, name: &str) -> Result<(Repository, Branch)> {
        let repository = self.create_repository(name).await?;
        let main = self.ensure_main_branch(repository.id).await?;
        Ok((repository, main))
    }

    pub async fn reset_repository(&self, name: &str) -> Result<(Repository, Branch)> {
        self.backend.delete_repository_by_name(name).await?;
        self.init_repository(name).await
    }

    pub async fn create_repository(&self, name: &str) -> Result<Repository> {
        self.backend.create_repository(name).await
    }

    pub async fn repository_by_name(&self, name: &str) -> Result<Repository> {
        self.backend.repository_by_name(name).await
    }

    pub async fn repository_by_id(&self, id: Uuid) -> Result<Repository> {
        self.backend.repository_by_id(id).await
    }

    pub async fn ensure_main_branch(&self, repository_id: Uuid) -> Result<Branch> {
        self.backend.ensure_main_branch(repository_id).await
    }

    pub async fn create_branch(
        &self,
        repository_id: Uuid,
        name: &str,
        base_name: &str,
    ) -> Result<Branch> {
        let base = self.branch_by_name(repository_id, base_name).await?;
        // The fork point (base_position) is immutable. If the branch already exists we return it
        // unchanged rather than advancing base_position to the current base length, which would
        // silently drop concurrent target operations from later merge analysis.
        let base_position = self.collect_branch_operations(base.id, None).await?.len() as i64;
        match self
            .backend
            .insert_branch(repository_id, name, base.id, base_position)
            .await?
        {
            Some(branch) => Ok(branch),
            None => self.branch_by_name(repository_id, name).await,
        }
    }

    pub async fn branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<Branch> {
        self.backend.branch_by_name(repository_id, name).await
    }

    pub async fn branch_by_id(&self, branch_id: Uuid) -> Result<Branch> {
        self.backend.branch_by_id(branch_id).await
    }

    pub async fn list_branches(&self, repository_id: Uuid) -> Result<Vec<Branch>> {
        self.backend.list_branches(repository_id).await
    }

    pub async fn create_task(&self, repository_id: Uuid, title: &str) -> Result<Task> {
        self.backend.create_task(repository_id, title).await
    }

    pub async fn create_changeset(
        &self,
        repository_id: Uuid,
        task_id: Uuid,
        branch_id: Uuid,
        title: &str,
        created_by: &str,
    ) -> Result<ChangeSet> {
        self.backend
            .create_changeset(repository_id, task_id, branch_id, title, created_by)
            .await
    }

    pub async fn append_operation(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        operation: Operation,
    ) -> Result<OperationRecord> {
        let op_type = operation.op_type();
        let payload = serde_json::to_value(&operation)?;
        self.backend
            .append_operation(repository_id, branch_id, changeset_id, op_type, payload)
            .await
    }

    pub async fn append_operations(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        operations: Vec<Operation>,
    ) -> Result<Vec<OperationRecord>> {
        let operations = operations
            .into_iter()
            .map(|operation| {
                let op_type = operation.op_type().to_string();
                Ok(backend::PendingOperation {
                    op_type,
                    payload: serde_json::to_value(operation)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.backend
            .append_operations(repository_id, branch_id, changeset_id, operations)
            .await
    }

    pub async fn append_operations_if_branch_position(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        expected_current_position: i64,
        operations: Vec<Operation>,
    ) -> Result<Vec<OperationRecord>> {
        let operations = operations
            .into_iter()
            .map(|operation| {
                let op_type = operation.op_type().to_string();
                Ok(backend::PendingOperation {
                    op_type,
                    payload: serde_json::to_value(operation)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.backend
            .append_operations_if_branch_position(
                repository_id,
                branch_id,
                changeset_id,
                expected_current_position,
                operations,
            )
            .await
    }

    pub async fn list_changesets(&self, repository_id: Uuid) -> Result<Vec<ChangeSet>> {
        self.backend.list_changesets(repository_id).await
    }

    pub async fn list_tasks(&self, repository_id: Uuid) -> Result<Vec<Task>> {
        self.backend.list_tasks(repository_id).await
    }

    pub async fn list_operations(&self, repository_id: Uuid) -> Result<Vec<OperationRecord>> {
        self.backend.list_operations(repository_id).await
    }

    pub async fn create_slice(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        base_position: i64,
        root_symbols: &[Uuid],
    ) -> Result<StoredSlice> {
        let branch = self.branch_by_id(branch_id).await?;
        if branch.repository_id != repository_id {
            anyhow::bail!("branch {branch_id} does not belong to repository {repository_id}");
        }
        self.backend
            .insert_slice(
                repository_id,
                branch_id,
                base_position,
                serde_json::to_value(root_symbols)?,
            )
            .await
    }

    pub async fn slice_by_id(&self, slice_id: Uuid) -> Result<StoredSlice> {
        self.backend.slice_by_id(slice_id).await
    }

    pub async fn list_own_operations(
        &self,
        branch_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<OperationRecord>> {
        self.backend.list_own_operations(branch_id, limit).await
    }

    pub async fn collect_branch_operations(
        &self,
        branch_id: Uuid,
        visible_limit: Option<i64>,
    ) -> Result<Vec<OperationRecord>> {
        self.collect_branch_operations_inner(branch_id, visible_limit)
            .await
    }

    fn collect_branch_operations_inner(
        &self,
        branch_id: Uuid,
        visible_limit: Option<i64>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<OperationRecord>>> + Send + '_>,
    > {
        Box::pin(async move {
            let branch = self.branch_by_id(branch_id).await?;
            let mut operations = if let Some(base_branch_id) = branch.base_branch_id {
                self.collect_branch_operations_inner(base_branch_id, Some(branch.base_position))
                    .await?
            } else {
                Vec::new()
            };

            if let Some(limit) = visible_limit {
                if operations.len() as i64 >= limit {
                    operations.truncate(limit as usize);
                    return Ok(operations);
                }
                let own_limit = limit - operations.len() as i64;
                operations.extend(self.list_own_operations(branch_id, Some(own_limit)).await?);
            } else {
                operations.extend(self.list_own_operations(branch_id, None).await?);
            }

            Ok(operations)
        })
    }

    pub async fn add_attachment(
        &self,
        repository_id: Uuid,
        entity_type: &str,
        entity_id: Uuid,
        attachment_type: &str,
        payload: Value,
    ) -> Result<Attachment> {
        self.backend
            .add_attachment(
                repository_id,
                entity_type,
                entity_id,
                attachment_type,
                payload,
            )
            .await
    }

    pub async fn list_source_file_snapshots(
        &self,
        branch_id: Uuid,
    ) -> Result<Vec<SourceFileSnapshot>> {
        self.backend.list_source_file_snapshots(branch_id).await
    }

    pub async fn replace_source_file_snapshots(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        snapshots: Vec<PendingSourceFileSnapshot>,
    ) -> Result<()> {
        self.backend
            .replace_source_file_snapshots(repository_id, branch_id, snapshots)
            .await
    }
}
