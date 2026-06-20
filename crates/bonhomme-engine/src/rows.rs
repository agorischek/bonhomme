use crate::{Attachment, SourceFileSnapshot, StoredSlice};
use anyhow::Result;
use bonhomme_core::{Branch, ChangeSet, OperationRecord, Repository, Task};
use chrono::{DateTime, Utc};
use serde_json::Value;
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
    pub(crate) graph: Value,
    pub(crate) rendered_files: Value,
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

pub(crate) fn operation_fingerprint(operations: &[OperationRecord]) -> String {
    operations
        .iter()
        .map(|operation| format!("{}:{}", operation.id, operation.position))
        .collect::<Vec<_>>()
        .join("|")
}
