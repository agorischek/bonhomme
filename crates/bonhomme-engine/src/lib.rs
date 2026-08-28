mod backend;
mod conflict;
mod materialization;
mod materialization_scale;
mod merge;
mod models;
mod rows;

#[cfg(test)]
mod storage_conformance_tests;
#[cfg(test)]
mod turso_tests;

use anyhow::Result;
use bonhomme_core::{
    Branch, ChangeSet, LanguagePlugin, Operation, OperationRecord, Repository, Task,
};
use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use backend::{AgentJobLease, LeaseAgentJobRequest, StorageBackend};
pub use conflict::ConflictViews;
pub use materialization_scale::{MaterializationScaleReport, run_materialization_scale_benchmark};
pub use models::{
    AgentJob, AgentJobSpec, AgentJobState, ApplySliceResult, Attachment, CacheStatus,
    CheckpointRejection, ConflictKind, ConflictPositions, ConflictRecord, ConflictState,
    LeasedAgentJob, MaterializationEvidence, MaterializedBranch, MaterializedGraph, MergeResult,
    PendingSourceFileSnapshot, ResolveConflictResult, SourceFileSnapshot, StillConflictingReason,
    StoredSlice,
};

pub const DEFAULT_DATABASE_URL: &str = "postgres://bonhomme:bonhomme@localhost:54329/bonhomme";
const MAX_AGENT_JOB_TENANT_KEY_BYTES: usize = 128;
const MAX_AGENT_JOB_KIND_BYTES: usize = 128;
const MAX_AGENT_JOB_IDEMPOTENCY_KEY_BYTES: usize = 256;
const MAX_AGENT_JOB_OWNER_BYTES: usize = 128;
const MAX_AGENT_JOB_ERROR_BYTES: usize = 16 * 1024;
const MAX_AGENT_JOB_ATTEMPTS: i64 = 100;
const MAX_AGENT_JOB_RETRY_BACKOFF_SECONDS: i64 = 86_400;
const MAX_AGENT_JOB_BACKOFF_SECONDS: i64 = 604_800;

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
        self.backend.reset_repository_by_name(name).await
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

    pub async fn delete_branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<()> {
        self.backend
            .delete_branch_by_name(repository_id, name)
            .await
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

    /// Enqueue a durable agent job. Reusing an idempotency key in the same tenant returns the
    /// original job only when the repository and immutable request fields match; conflicting reuse
    /// anywhere in that tenant is rejected.
    pub async fn enqueue_agent_job(
        &self,
        repository_id: Uuid,
        spec: AgentJobSpec,
    ) -> Result<AgentJob> {
        validate_agent_job_spec(&spec)?;
        let expected = spec.clone();
        let request_fingerprint = agent_job_request_fingerprint(repository_id, &spec)?;
        let job = self
            .backend
            .enqueue_agent_job(repository_id, spec, request_fingerprint, Utc::now())
            .await?;
        if job.tenant_key != expected.tenant_key
            || job.repository_id != repository_id
            || job.kind != expected.kind
            || job.payload != expected.payload
            || job.max_attempts != expected.max_attempts
            || job.retry_backoff_seconds != expected.retry_backoff_seconds
            || job.max_backoff_seconds != expected.max_backoff_seconds
        {
            anyhow::bail!(
                "idempotency key {} is already used by a different job request",
                expected.idempotency_key
            );
        }
        Ok(job)
    }

    pub async fn agent_job_by_id(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
    ) -> Result<AgentJob> {
        validate_tenant(tenant_key)?;
        self.backend
            .agent_job_by_id(tenant_key, repository_id, job_id)
            .await
    }

    pub async fn list_agent_jobs(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
    ) -> Result<Vec<AgentJob>> {
        validate_tenant(tenant_key)?;
        self.backend
            .list_agent_jobs(tenant_key, repository_id)
            .await
    }

    /// Atomically lease the oldest available job, optionally limited to a job kind.
    pub async fn lease_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        owner: &str,
        kind: Option<&str>,
        lease_duration: std::time::Duration,
    ) -> Result<Option<LeasedAgentJob>> {
        validate_tenant(tenant_key)?;
        validate_worker_and_duration(owner, lease_duration)?;
        if let Some(kind) = kind {
            validate_bounded_value("agent job kind", kind, MAX_AGENT_JOB_KIND_BYTES)?;
        }
        let now = Utc::now();
        let expires_at = now + chrono::Duration::from_std(lease_duration)?;
        let lease_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let digest = agent_job_lease_token_digest(&lease_token);
        let job = self
            .backend
            .lease_agent_job(LeaseAgentJobRequest {
                tenant_key,
                repository_id,
                owner,
                lease_token_digest: &digest,
                kind,
                now,
                lease_expires_at: expires_at,
            })
            .await?;
        Ok(job.map(|job| LeasedAgentJob { job, lease_token }))
    }

    pub async fn heartbeat_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
        owner: &str,
        lease_token: &str,
        lease_duration: std::time::Duration,
    ) -> Result<AgentJob> {
        validate_tenant(tenant_key)?;
        validate_worker_and_duration(owner, lease_duration)?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::from_std(lease_duration)?;
        let digest = validated_lease_token_digest(lease_token)?;
        self.backend
            .heartbeat_agent_job(
                AgentJobLease {
                    tenant_key,
                    repository_id,
                    job_id,
                    owner,
                    lease_token_digest: &digest,
                    now,
                },
                expires_at,
            )
            .await
    }

    pub async fn succeed_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
        owner: &str,
        lease_token: &str,
        result: Value,
    ) -> Result<AgentJob> {
        validate_tenant(tenant_key)?;
        validate_worker(owner)?;
        let digest = validated_lease_token_digest(lease_token)?;
        self.backend
            .succeed_agent_job(
                AgentJobLease {
                    tenant_key,
                    repository_id,
                    job_id,
                    owner,
                    lease_token_digest: &digest,
                    now: Utc::now(),
                },
                result,
            )
            .await
    }

    /// Record an attempt failure. The backend atomically requeues with capped exponential
    /// backoff, or transitions to terminal `failed` after `max_attempts` is reached.
    pub async fn fail_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
        owner: &str,
        lease_token: &str,
        error: &str,
    ) -> Result<AgentJob> {
        validate_tenant(tenant_key)?;
        validate_worker(owner)?;
        let digest = validated_lease_token_digest(lease_token)?;
        let error = bounded_agent_job_error(error)?;
        self.backend
            .fail_agent_job(
                AgentJobLease {
                    tenant_key,
                    repository_id,
                    job_id,
                    owner,
                    lease_token_digest: &digest,
                    now: Utc::now(),
                },
                &error,
            )
            .await
    }

    pub async fn cancel_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
    ) -> Result<AgentJob> {
        validate_tenant(tenant_key)?;
        self.backend
            .cancel_agent_job(tenant_key, repository_id, job_id, Utc::now())
            .await
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

fn validate_agent_job_spec(spec: &AgentJobSpec) -> Result<()> {
    validate_tenant(&spec.tenant_key)?;
    validate_bounded_value("agent job kind", &spec.kind, MAX_AGENT_JOB_KIND_BYTES)?;
    validate_bounded_value(
        "agent job idempotency key",
        &spec.idempotency_key,
        MAX_AGENT_JOB_IDEMPOTENCY_KEY_BYTES,
    )?;
    if !(1..=MAX_AGENT_JOB_ATTEMPTS).contains(&spec.max_attempts) {
        anyhow::bail!("agent job max_attempts must be between 1 and {MAX_AGENT_JOB_ATTEMPTS}");
    }
    if !(0..=MAX_AGENT_JOB_RETRY_BACKOFF_SECONDS).contains(&spec.retry_backoff_seconds) {
        anyhow::bail!(
            "agent job retry_backoff_seconds must be between 0 and {MAX_AGENT_JOB_RETRY_BACKOFF_SECONDS}"
        );
    }
    if !(spec.retry_backoff_seconds..=MAX_AGENT_JOB_BACKOFF_SECONDS)
        .contains(&spec.max_backoff_seconds)
    {
        anyhow::bail!(
            "agent job max_backoff_seconds must be between retry_backoff_seconds and {MAX_AGENT_JOB_BACKOFF_SECONDS}"
        );
    }
    Ok(())
}

fn agent_job_request_fingerprint(repository_id: Uuid, spec: &AgentJobSpec) -> Result<String> {
    let encoded = serde_json::to_vec(&(
        repository_id,
        &spec.tenant_key,
        &spec.kind,
        &spec.idempotency_key,
        &spec.payload,
        spec.max_attempts,
        spec.retry_backoff_seconds,
        spec.max_backoff_seconds,
        spec.available_at
            .as_ref()
            .map(|value| value.timestamp_micros()),
    ))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn validate_tenant(tenant_key: &str) -> Result<()> {
    validate_bounded_value(
        "agent job tenant key",
        tenant_key,
        MAX_AGENT_JOB_TENANT_KEY_BYTES,
    )
}

fn validate_worker(owner: &str) -> Result<()> {
    validate_bounded_value("agent job lease owner", owner, MAX_AGENT_JOB_OWNER_BYTES)
}

fn validate_bounded_value(label: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{label} cannot be empty");
    }
    if value.len() > max_bytes {
        anyhow::bail!("{label} cannot exceed {max_bytes} bytes");
    }
    Ok(())
}

fn bounded_agent_job_error(error: &str) -> Result<String> {
    if error.trim().is_empty() {
        anyhow::bail!("agent job error cannot be empty");
    }
    if error.len() <= MAX_AGENT_JOB_ERROR_BYTES {
        return Ok(error.to_string());
    }

    const TRUNCATION_MARKER: &str = "\n[diagnostic truncated]";
    let mut end = MAX_AGENT_JOB_ERROR_BYTES - TRUNCATION_MARKER.len();
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = String::with_capacity(MAX_AGENT_JOB_ERROR_BYTES);
    bounded.push_str(&error[..end]);
    bounded.push_str(TRUNCATION_MARKER);
    debug_assert!(bounded.len() <= MAX_AGENT_JOB_ERROR_BYTES);
    Ok(bounded)
}

fn validate_worker_and_duration(owner: &str, duration: std::time::Duration) -> Result<()> {
    validate_worker(owner)?;
    if duration.is_zero() {
        anyhow::bail!("agent job lease duration must be positive");
    }
    Ok(())
}

fn validated_lease_token_digest(token: &str) -> Result<String> {
    validate_bounded_value("agent job lease token", token, 128)?;
    Ok(agent_job_lease_token_digest(token))
}

fn agent_job_lease_token_digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
