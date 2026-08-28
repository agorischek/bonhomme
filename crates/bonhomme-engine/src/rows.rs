use crate::{
    AgentJob, AgentJobState, Attachment, ConflictKind, ConflictRecord, ConflictState,
    SourceFileSnapshot, StoredSlice,
};
use anyhow::Result;
use bonhomme_core::{
    Branch, ChangeSet, MergeConflict, Operation, OperationRecord, Repository, Task,
    canonical_json_bytes,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(FromRow)]
pub(crate) struct RepositoryRow {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
}

impl From<RepositoryRow> for Repository {
    fn from(row: RepositoryRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
pub(crate) struct BranchRow {
    id: Uuid,
    repository_id: Uuid,
    name: String,
    base_branch_id: Option<Uuid>,
    base_position: i64,
    created_at: DateTime<Utc>,
}

impl From<BranchRow> for Branch {
    fn from(row: BranchRow) -> Self {
        Self {
            id: row.id,
            repository_id: row.repository_id,
            name: row.name,
            base_branch_id: row.base_branch_id,
            base_position: row.base_position,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
pub(crate) struct TaskRow {
    id: Uuid,
    repository_id: Uuid,
    title: String,
    created_at: DateTime<Utc>,
}

impl From<TaskRow> for Task {
    fn from(row: TaskRow) -> Self {
        Self {
            id: row.id,
            repository_id: row.repository_id,
            title: row.title,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
pub(crate) struct ChangeSetRow {
    id: Uuid,
    repository_id: Uuid,
    task_id: Uuid,
    branch_id: Uuid,
    title: String,
    created_by: String,
    created_at: DateTime<Utc>,
}

impl From<ChangeSetRow> for ChangeSet {
    fn from(row: ChangeSetRow) -> Self {
        Self {
            id: row.id,
            repository_id: row.repository_id,
            task_id: row.task_id,
            branch_id: row.branch_id,
            title: row.title,
            created_by: row.created_by,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
pub(crate) struct OperationRow {
    id: Uuid,
    repository_id: Uuid,
    branch_id: Uuid,
    changeset_id: Uuid,
    position: i64,
    payload: Value,
    created_at: DateTime<Utc>,
}

impl TryFrom<OperationRow> for OperationRecord {
    type Error = anyhow::Error;

    fn try_from(row: OperationRow) -> Result<Self> {
        let operation = serde_json::from_value(row.payload)?;
        Ok(Self {
            id: row.id,
            repository_id: row.repository_id,
            branch_id: row.branch_id,
            changeset_id: row.changeset_id,
            position: row.position,
            operation,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
pub(crate) struct AttachmentRow {
    id: Uuid,
    repository_id: Uuid,
    entity_type: String,
    entity_id: Uuid,
    attachment_type: String,
    payload: Value,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
pub(crate) struct SliceRow {
    id: Uuid,
    repository_id: Uuid,
    branch_id: Uuid,
    base_position: i64,
    root_symbols: Value,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
pub(crate) struct GraphCacheRow {
    pub(crate) operation_count: i64,
    pub(crate) operation_fingerprint: String,
    pub(crate) graph: Value,
}

#[derive(FromRow)]
pub(crate) struct SourceFileSnapshotRow {
    repository_id: Uuid,
    branch_id: Uuid,
    path: String,
    content_hash: String,
    byte_len: i64,
    handler: String,
    file_symbol_id: Option<Uuid>,
    last_import_position: i64,
    importer_version: String,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
pub(crate) struct ConflictRow {
    id: Uuid,
    repository_id: Uuid,
    kind: String,
    state: String,
    source_branch_id: Option<Uuid>,
    target_branch_id: Option<Uuid>,
    source_branch_name: String,
    target_branch_name: String,
    slice_id: Option<Uuid>,
    source_changeset_id: Option<Uuid>,
    base_position: i64,
    target_position: i64,
    conflicts: Value,
    proposed_operations: Value,
    resolution_changeset_id: Option<Uuid>,
    acknowledged_drop: bool,
    created_by: String,
    resolved_by: Option<String>,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
pub(crate) struct AgentJobRow {
    id: Uuid,
    tenant_key: String,
    repository_id: Uuid,
    kind: String,
    idempotency_key: String,
    payload: Value,
    state: String,
    attempt_count: i64,
    max_attempts: i64,
    retry_backoff_seconds: i64,
    max_backoff_seconds: i64,
    available_at: DateTime<Utc>,
    lease_owner: Option<String>,
    lease_expires_at: Option<DateTime<Utc>>,
    result: Option<Value>,
    error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

impl TryFrom<SliceRow> for StoredSlice {
    type Error = anyhow::Error;

    fn try_from(row: SliceRow) -> Result<Self> {
        let root_symbols = serde_json::from_value(row.root_symbols)?;
        Ok(Self {
            id: row.id,
            repository_id: row.repository_id,
            branch_id: row.branch_id,
            base_position: row.base_position,
            root_symbols,
            created_at: row.created_at,
        })
    }
}

impl From<AttachmentRow> for Attachment {
    fn from(row: AttachmentRow) -> Self {
        Self {
            id: row.id,
            repository_id: row.repository_id,
            entity_type: row.entity_type,
            entity_id: row.entity_id,
            attachment_type: row.attachment_type,
            payload: row.payload,
            created_at: row.created_at,
        }
    }
}

impl From<SourceFileSnapshotRow> for SourceFileSnapshot {
    fn from(row: SourceFileSnapshotRow) -> Self {
        Self {
            repository_id: row.repository_id,
            branch_id: row.branch_id,
            path: row.path,
            content_hash: row.content_hash,
            byte_len: row.byte_len,
            handler: row.handler,
            file_symbol_id: row.file_symbol_id,
            last_import_position: row.last_import_position,
            importer_version: row.importer_version,
            updated_at: row.updated_at,
        }
    }
}

impl TryFrom<ConflictRow> for ConflictRecord {
    type Error = anyhow::Error;

    fn try_from(row: ConflictRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            repository_id: row.repository_id,
            kind: row.kind.parse::<ConflictKind>()?,
            state: row.state.parse::<ConflictState>()?,
            source_branch_id: row.source_branch_id,
            target_branch_id: row.target_branch_id,
            source_branch_name: row.source_branch_name,
            target_branch_name: row.target_branch_name,
            slice_id: row.slice_id,
            source_changeset_id: row.source_changeset_id,
            base_position: row.base_position,
            target_position: row.target_position,
            conflicts: serde_json::from_value::<Vec<MergeConflict>>(row.conflicts)?,
            proposed_operations: serde_json::from_value::<Vec<Operation>>(row.proposed_operations)?,
            resolution_changeset_id: row.resolution_changeset_id,
            acknowledged_drop: row.acknowledged_drop,
            created_by: row.created_by,
            resolved_by: row.resolved_by,
            created_at: row.created_at,
            resolved_at: row.resolved_at,
        })
    }
}

impl TryFrom<AgentJobRow> for AgentJob {
    type Error = anyhow::Error;

    fn try_from(row: AgentJobRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            tenant_key: row.tenant_key,
            repository_id: row.repository_id,
            kind: row.kind,
            idempotency_key: row.idempotency_key,
            payload: row.payload,
            state: row.state.parse::<AgentJobState>()?,
            attempt_count: row.attempt_count,
            max_attempts: row.max_attempts,
            retry_backoff_seconds: row.retry_backoff_seconds,
            max_backoff_seconds: row.max_backoff_seconds,
            available_at: row.available_at,
            lease_owner: row.lease_owner,
            lease_expires_at: row.lease_expires_at,
            result: row.result,
            error: row.error,
            created_at: row.created_at,
            updated_at: row.updated_at,
            completed_at: row.completed_at,
        })
    }
}

pub(crate) fn operation_fingerprint(operations: &[OperationRecord]) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"bonhomme-operation-prefix-v1\0");
    for operation in operations {
        digest.update(operation.id.as_bytes());
        digest.update(operation.position.to_be_bytes());
        let payload = canonical_json_bytes(&operation.operation)?;
        digest.update((payload.len() as u64).to_be_bytes());
        digest.update(payload);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

#[cfg(test)]
mod fingerprint_tests {
    use bonhomme_core::Operation;
    use chrono::{DateTime, Utc};

    use super::*;

    fn record(id: u128, position: i64, operation: Operation) -> OperationRecord {
        OperationRecord {
            id: Uuid::from_u128(id),
            repository_id: Uuid::from_u128(100),
            branch_id: Uuid::from_u128(101),
            changeset_id: Uuid::from_u128(102),
            position,
            operation,
            created_at: DateTime::<Utc>::UNIX_EPOCH,
        }
    }

    #[test]
    fn fingerprint_binds_order_identity_position_and_payload() {
        let first = record(
            1,
            1,
            Operation::DeleteReference {
                reference_id: Uuid::from_u128(8),
            },
        );
        let mut changed_payload = first.clone();
        changed_payload.operation = Operation::DeleteReference {
            reference_id: Uuid::from_u128(9),
        };
        let mut changed_position = first.clone();
        changed_position.position = 2;

        let baseline = operation_fingerprint(std::slice::from_ref(&first)).unwrap();
        assert!(baseline.starts_with("sha256:"));
        assert_ne!(baseline, operation_fingerprint(&[changed_payload]).unwrap());
        assert_ne!(
            baseline,
            operation_fingerprint(&[changed_position]).unwrap()
        );
        assert_ne!(baseline, operation_fingerprint(&[]).unwrap());
    }
}
