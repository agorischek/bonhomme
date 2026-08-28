//! Postgres backend (sqlx). The original engine SQL, moved verbatim behind `StorageBackend`.
use anyhow::{Context, Result, bail};
use bonhomme_core::{Branch, ChangeSet, OperationRecord, Repository, Task};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{
    AgentJobLease, BranchAdvanced, CommittedConflictResolution, CommittedOperations,
    LeaseAgentJobRequest, PendingAttachment, PendingConflictResolutionCommit, PendingOperation,
    PendingOperationCommit, StorageBackend, StoredGraphCheckpoint,
};
use crate::{
    AgentJob, AgentJobSpec, Attachment, ConflictRecord, ConflictState, PendingSourceFileSnapshot,
    SourceFileSnapshot, StoredSlice,
    models::PendingConflict,
    rows::{
        AgentJobRow, AttachmentRow, BranchRow, ChangeSetRow, ConflictRow, GraphCacheRow,
        OperationRow, RepositoryRow, SliceRow, SourceFileSnapshotRow, TaskRow,
    },
};

pub(crate) struct PostgresBackend {
    pool: PgPool,
}

impl PostgresBackend {
    pub(crate) async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(12)
            .connect(database_url)
            .await
            .with_context(|| format!("failed to connect to Postgres at {database_url}"))?;
        Ok(Self { pool })
    }

    async fn append_operations_locked(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        expected_current_position: Option<i64>,
        operations: Vec<PendingOperation>,
    ) -> Result<Vec<OperationRecord>> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
            .bind(branch_id.to_string())
            .execute(&mut *tx)
            .await?;
        let current_position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), 0) FROM operations WHERE branch_id = $1",
        )
        .bind(branch_id)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(expected) = expected_current_position
            && current_position != expected
        {
            bail!(
                "branch {branch_id} advanced from position {expected} to {current_position}; retry merge"
            );
        }

        let mut appended = Vec::with_capacity(operations.len());
        for (next_position, operation) in (current_position + 1..).zip(operations) {
            let row = sqlx::query_as::<_, OperationRow>(
                r#"
                INSERT INTO operations (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(repository_id)
            .bind(branch_id)
            .bind(changeset_id)
            .bind(next_position)
            .bind(operation.op_type)
            .bind(operation.payload)
            .fetch_one(&mut *tx)
            .await?;
            appended.push(row.try_into()?);
        }
        tx.commit().await?;
        Ok(appended)
    }
}

#[async_trait::async_trait]
impl StorageBackend for PostgresBackend {
    async fn run_migrations(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context("failed to run database migrations")?;
        Ok(())
    }

    async fn reset_repository_by_name(&self, name: &str) -> Result<(Repository, Branch)> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM repositories WHERE name = $1")
            .bind(name)
            .execute(&mut *tx)
            .await?;
        let repository: Repository = sqlx::query_as::<_, RepositoryRow>(
            "INSERT INTO repositories (id, name) VALUES ($1, $2)
             RETURNING id, name, created_at",
        )
        .bind(Uuid::new_v4())
        .bind(name)
        .fetch_one(&mut *tx)
        .await?
        .into();
        let main: Branch = sqlx::query_as::<_, BranchRow>(
            "INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
             VALUES ($1, $2, 'main', NULL, 0)
             RETURNING id, repository_id, name, base_branch_id, base_position, created_at",
        )
        .bind(Uuid::new_v4())
        .bind(repository.id)
        .fetch_one(&mut *tx)
        .await?
        .into();
        tx.commit().await?;
        Ok((repository, main))
    }

    async fn create_repository(&self, name: &str) -> Result<Repository> {
        let row = sqlx::query_as::<_, RepositoryRow>(
            r#"
            INSERT INTO repositories (id, name)
            VALUES ($1, $2)
            ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
            RETURNING id, name, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    async fn repository_by_name(&self, name: &str) -> Result<Repository> {
        let row = sqlx::query_as::<_, RepositoryRow>(
            "SELECT id, name, created_at FROM repositories WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("repository {name} does not exist"))?;
        Ok(row.into())
    }

    async fn repository_by_id(&self, id: Uuid) -> Result<Repository> {
        let row = sqlx::query_as::<_, RepositoryRow>(
            "SELECT id, name, created_at FROM repositories WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("repository {id} does not exist"))?;
        Ok(row.into())
    }

    async fn ensure_main_branch(&self, repository_id: Uuid) -> Result<Branch> {
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
            VALUES ($1, $2, 'main', NULL, 0)
            ON CONFLICT (repository_id, name) DO UPDATE SET name = EXCLUDED.name
            RETURNING id, repository_id, name, base_branch_id, base_position, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    async fn insert_branch(
        &self,
        repository_id: Uuid,
        name: &str,
        base_branch_id: Uuid,
        base_position: i64,
    ) -> Result<Option<Branch>> {
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (repository_id, name) DO NOTHING
            RETURNING id, repository_id, name, base_branch_id, base_position, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .bind(name)
        .bind(base_branch_id)
        .bind(base_position)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    async fn delete_branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let branch_id: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM branches WHERE repository_id = $1 AND name = $2")
                .bind(repository_id)
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(branch_id) = branch_id else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
            .bind(branch_id.to_string())
            .execute(&mut *tx)
            .await?;
        let still_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM branches WHERE id = $1 FOR UPDATE")
                .bind(branch_id)
                .fetch_optional(&mut *tx)
                .await?;
        if still_exists.is_none() {
            tx.commit().await?;
            return Ok(());
        }
        let active_conflict: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, state FROM conflicts
             WHERE state IN ('open', 'resolving')
               AND (source_branch_id = $1 OR target_branch_id = $1)
             ORDER BY created_at, id
             LIMIT 1",
        )
        .bind(branch_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((conflict_id, state)) = active_conflict {
            bail!("cannot delete branch {name}; conflict {conflict_id} is still {state}");
        }
        sqlx::query("DELETE FROM branches WHERE id = $1")
            .bind(branch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<Branch> {
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            SELECT id, repository_id, name, base_branch_id, base_position, created_at
            FROM branches WHERE repository_id = $1 AND name = $2
            "#,
        )
        .bind(repository_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("branch {name} does not exist"))?;
        Ok(row.into())
    }

    async fn branch_by_id(&self, branch_id: Uuid) -> Result<Branch> {
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            SELECT id, repository_id, name, base_branch_id, base_position, created_at
            FROM branches WHERE id = $1
            "#,
        )
        .bind(branch_id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("branch {branch_id} does not exist"))?;
        Ok(row.into())
    }

    async fn list_branches(&self, repository_id: Uuid) -> Result<Vec<Branch>> {
        let rows = sqlx::query_as::<_, BranchRow>(
            r#"
            SELECT id, repository_id, name, base_branch_id, base_position, created_at
            FROM branches WHERE repository_id = $1 ORDER BY created_at, name
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn create_task(&self, repository_id: Uuid, title: &str) -> Result<Task> {
        let row = sqlx::query_as::<_, TaskRow>(
            r#"
            INSERT INTO tasks (id, repository_id, title)
            VALUES ($1, $2, $3)
            RETURNING id, repository_id, title, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .bind(title)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    async fn list_tasks(&self, repository_id: Uuid) -> Result<Vec<Task>> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT id, repository_id, title, created_at FROM tasks WHERE repository_id = $1 ORDER BY created_at, id",
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn create_changeset(
        &self,
        repository_id: Uuid,
        task_id: Uuid,
        branch_id: Uuid,
        title: &str,
        created_by: &str,
    ) -> Result<ChangeSet> {
        let row = sqlx::query_as::<_, ChangeSetRow>(
            r#"
            INSERT INTO changesets (id, repository_id, task_id, branch_id, title, created_by)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, repository_id, task_id, branch_id, title, created_by, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .bind(task_id)
        .bind(branch_id)
        .bind(title)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    async fn list_changesets(&self, repository_id: Uuid) -> Result<Vec<ChangeSet>> {
        let rows = sqlx::query_as::<_, ChangeSetRow>(
            r#"
            SELECT id, repository_id, task_id, branch_id, title, created_by, created_at
            FROM changesets WHERE repository_id = $1 ORDER BY created_at, id
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn append_operation(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        op_type: &str,
        payload: Value,
    ) -> Result<OperationRecord> {
        let mut tx = self.pool.begin().await?;
        // Serialize position allocation per branch with a transaction-scoped advisory lock so two
        // concurrent appends cannot both read the same MAX(position) and collide on the
        // UNIQUE(branch_id, position) constraint.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
            .bind(branch_id.to_string())
            .execute(&mut *tx)
            .await?;
        let next_position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM operations WHERE branch_id = $1",
        )
        .bind(branch_id)
        .fetch_one(&mut *tx)
        .await?;
        let row = sqlx::query_as::<_, OperationRow>(
            r#"
            INSERT INTO operations (id, repository_id, branch_id, changeset_id, position, op_type, payload)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .bind(branch_id)
        .bind(changeset_id)
        .bind(next_position)
        .bind(op_type)
        .bind(payload)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        row.try_into()
    }

    async fn append_operations(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        operations: Vec<PendingOperation>,
    ) -> Result<Vec<OperationRecord>> {
        self.append_operations_locked(repository_id, branch_id, changeset_id, None, operations)
            .await
    }

    async fn append_operations_if_branch_position(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        expected_current_position: i64,
        operations: Vec<PendingOperation>,
    ) -> Result<Vec<OperationRecord>> {
        self.append_operations_locked(
            repository_id,
            branch_id,
            changeset_id,
            Some(expected_current_position),
            operations,
        )
        .await
    }

    async fn commit_operations(
        &self,
        commit: PendingOperationCommit,
    ) -> Result<CommittedOperations> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
            .bind(commit.branch_id.to_string())
            .execute(&mut *tx)
            .await?;
        let current_position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), 0) FROM operations WHERE branch_id = $1",
        )
        .bind(commit.branch_id)
        .fetch_one(&mut *tx)
        .await?;
        if current_position != commit.expected_current_position {
            return Err(BranchAdvanced {
                branch_id: commit.branch_id,
                expected: commit.expected_current_position,
                actual: current_position,
            }
            .into());
        }

        let task_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tasks (id, repository_id, title) VALUES ($1, $2, $3)")
            .bind(task_id)
            .bind(commit.repository_id)
            .bind(commit.task_title)
            .execute(&mut *tx)
            .await?;
        let changeset: ChangeSet = sqlx::query_as::<_, ChangeSetRow>(
            "INSERT INTO changesets
                (id, repository_id, task_id, branch_id, title, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, repository_id, task_id, branch_id, title, created_by, created_at",
        )
        .bind(Uuid::new_v4())
        .bind(commit.repository_id)
        .bind(task_id)
        .bind(commit.branch_id)
        .bind(commit.changeset_title)
        .bind(commit.created_by)
        .fetch_one(&mut *tx)
        .await?
        .into();
        let mut operations = Vec::with_capacity(commit.operations.len());
        for (position, pending) in (current_position + 1..).zip(commit.operations) {
            let row = sqlx::query_as::<_, OperationRow>(
                "INSERT INTO operations
                    (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at",
            )
            .bind(Uuid::new_v4())
            .bind(commit.repository_id)
            .bind(commit.branch_id)
            .bind(changeset.id)
            .bind(position)
            .bind(pending.op_type)
            .bind(pending.payload)
            .fetch_one(&mut *tx)
            .await?;
            operations.push(row.try_into()?);
        }
        tx.commit().await?;
        Ok(CommittedOperations { operations })
    }

    async fn list_operations(&self, repository_id: Uuid) -> Result<Vec<OperationRecord>> {
        let rows = sqlx::query_as::<_, OperationRow>(
            r#"
            SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
            FROM operations WHERE repository_id = $1 ORDER BY created_at, branch_id, position
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn list_own_operations(
        &self,
        branch_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<OperationRecord>> {
        let rows = if let Some(limit) = limit {
            sqlx::query_as::<_, OperationRow>(
                r#"
                SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                FROM operations WHERE branch_id = $1 ORDER BY position LIMIT $2
                "#,
            )
            .bind(branch_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, OperationRow>(
                r#"
                SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                FROM operations WHERE branch_id = $1 ORDER BY position
                "#,
            )
            .bind(branch_id)
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn insert_slice(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        base_position: i64,
        root_symbols: Value,
    ) -> Result<StoredSlice> {
        let row = sqlx::query_as::<_, SliceRow>(
            r#"
            INSERT INTO slices (id, repository_id, branch_id, base_position, root_symbols)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, repository_id, branch_id, base_position, root_symbols, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .bind(branch_id)
        .bind(base_position)
        .bind(root_symbols)
        .fetch_one(&self.pool)
        .await?;
        row.try_into()
    }

    async fn slice_by_id(&self, slice_id: Uuid) -> Result<StoredSlice> {
        let row = sqlx::query_as::<_, SliceRow>(
            r#"
            SELECT id, repository_id, branch_id, base_position, root_symbols, created_at
            FROM slices WHERE id = $1
            "#,
        )
        .bind(slice_id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("slice {slice_id} does not exist"))?;
        row.try_into()
    }

    async fn insert_conflict_with_attachment(
        &self,
        conflict: PendingConflict,
        attachment: PendingAttachment,
    ) -> Result<ConflictRecord> {
        let mut tx = self.pool.begin().await?;
        let mut branch_ids = [conflict.source_branch_id, conflict.target_branch_id];
        branch_ids.sort_unstable();
        let branch_count = if branch_ids[0] == branch_ids[1] { 1 } else { 2 };
        for branch_id in branch_ids.into_iter().take(branch_count) {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
                .bind(branch_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        let row = sqlx::query_as::<_, ConflictRow>(
            r#"
            INSERT INTO conflicts (
                id, repository_id, kind, state,
                source_branch_id, target_branch_id,
                source_branch_name, target_branch_name,
                slice_id, source_changeset_id,
                base_position, target_position,
                conflicts, proposed_operations, created_by
            )
            VALUES ($1, $2, $3, 'open', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            RETURNING id, repository_id, kind, state,
                      source_branch_id, target_branch_id,
                      source_branch_name, target_branch_name,
                      slice_id, source_changeset_id,
                      base_position, target_position,
                      conflicts, proposed_operations,
                      resolution_changeset_id, acknowledged_drop,
                      created_by, resolved_by, created_at, resolved_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(conflict.repository_id)
        .bind(conflict.kind.as_str())
        .bind(conflict.source_branch_id)
        .bind(conflict.target_branch_id)
        .bind(conflict.source_branch_name)
        .bind(conflict.target_branch_name)
        .bind(conflict.slice_id)
        .bind(conflict.source_changeset_id)
        .bind(conflict.base_position)
        .bind(conflict.target_position)
        .bind(serde_json::to_value(conflict.conflicts)?)
        .bind(serde_json::to_value(conflict.proposed_operations)?)
        .bind(conflict.created_by)
        .fetch_one(&mut *tx)
        .await?;
        let record: ConflictRecord = row.try_into()?;
        sqlx::query(
            "INSERT INTO attachments
                (id, repository_id, entity_type, entity_id, attachment_type, payload)
             VALUES ($1, $2, 'conflict', $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(record.repository_id)
        .bind(record.id)
        .bind(attachment.attachment_type)
        .bind(attachment.payload)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    async fn list_conflicts(&self, repository_id: Uuid) -> Result<Vec<ConflictRecord>> {
        let rows = sqlx::query_as::<_, ConflictRow>(
            r#"
            SELECT id, repository_id, kind, state,
                   source_branch_id, target_branch_id,
                   source_branch_name, target_branch_name,
                   slice_id, source_changeset_id,
                   base_position, target_position,
                   conflicts, proposed_operations,
                   resolution_changeset_id, acknowledged_drop,
                   created_by, resolved_by, created_at, resolved_at
            FROM conflicts
            WHERE repository_id = $1
            ORDER BY created_at DESC, id
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn conflict_by_id(&self, conflict_id: Uuid) -> Result<ConflictRecord> {
        let row = sqlx::query_as::<_, ConflictRow>(
            r#"
            SELECT id, repository_id, kind, state,
                   source_branch_id, target_branch_id,
                   source_branch_name, target_branch_name,
                   slice_id, source_changeset_id,
                   base_position, target_position,
                   conflicts, proposed_operations,
                   resolution_changeset_id, acknowledged_drop,
                   created_by, resolved_by, created_at, resolved_at
            FROM conflicts
            WHERE id = $1
            "#,
        )
        .bind(conflict_id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("conflict {conflict_id} does not exist"))?;
        row.try_into()
    }

    async fn update_conflict_state(
        &self,
        conflict_id: Uuid,
        state: ConflictState,
    ) -> Result<ConflictRecord> {
        let row = sqlx::query_as::<_, ConflictRow>(
            r#"
            UPDATE conflicts
            SET state = $2
            WHERE id = $1
            RETURNING id, repository_id, kind, state,
                      source_branch_id, target_branch_id,
                      source_branch_name, target_branch_name,
                      slice_id, source_changeset_id,
                      base_position, target_position,
                      conflicts, proposed_operations,
                      resolution_changeset_id, acknowledged_drop,
                      created_by, resolved_by, created_at, resolved_at
            "#,
        )
        .bind(conflict_id)
        .bind(state.as_str())
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("conflict {conflict_id} does not exist"))?;
        row.try_into()
    }

    async fn commit_conflict_resolution(
        &self,
        resolution: PendingConflictResolutionCommit,
    ) -> Result<CommittedConflictResolution> {
        let PendingConflictResolutionCommit {
            commit,
            conflict_id,
            attachment,
            resolved_by,
            acknowledged_drop,
        } = resolution;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
            .bind(commit.branch_id.to_string())
            .execute(&mut *tx)
            .await?;
        let current_position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), 0) FROM operations WHERE branch_id = $1",
        )
        .bind(commit.branch_id)
        .fetch_one(&mut *tx)
        .await?;
        if current_position != commit.expected_current_position {
            return Err(BranchAdvanced {
                branch_id: commit.branch_id,
                expected: commit.expected_current_position,
                actual: current_position,
            }
            .into());
        }

        let (conflict_repository_id, target_branch_id, state): (Uuid, Uuid, String) =
            sqlx::query_as(
                "SELECT repository_id, target_branch_id, state
                 FROM conflicts WHERE id = $1 FOR UPDATE",
            )
            .bind(conflict_id)
            .fetch_one(&mut *tx)
            .await
            .with_context(|| format!("conflict {conflict_id} does not exist"))?;
        if conflict_repository_id != commit.repository_id
            || target_branch_id != commit.branch_id
            || !matches!(state.as_str(), "open" | "resolving")
        {
            bail!("conflict {conflict_id} is not resolvable on the requested branch");
        }

        let task_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tasks (id, repository_id, title) VALUES ($1, $2, $3)")
            .bind(task_id)
            .bind(commit.repository_id)
            .bind(commit.task_title)
            .execute(&mut *tx)
            .await?;
        let changeset: ChangeSet = sqlx::query_as::<_, ChangeSetRow>(
            "INSERT INTO changesets
                (id, repository_id, task_id, branch_id, title, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, repository_id, task_id, branch_id, title, created_by, created_at",
        )
        .bind(Uuid::new_v4())
        .bind(commit.repository_id)
        .bind(task_id)
        .bind(commit.branch_id)
        .bind(commit.changeset_title)
        .bind(commit.created_by)
        .fetch_one(&mut *tx)
        .await?
        .into();
        let mut operations = Vec::with_capacity(commit.operations.len());
        for (position, pending) in (current_position + 1..).zip(commit.operations) {
            let row = sqlx::query_as::<_, OperationRow>(
                "INSERT INTO operations
                    (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at",
            )
            .bind(Uuid::new_v4())
            .bind(commit.repository_id)
            .bind(commit.branch_id)
            .bind(changeset.id)
            .bind(position)
            .bind(pending.op_type)
            .bind(pending.payload)
            .fetch_one(&mut *tx)
            .await?;
            operations.push(row.try_into()?);
        }
        sqlx::query(
            "INSERT INTO attachments
                (id, repository_id, entity_type, entity_id, attachment_type, payload)
             VALUES ($1, $2, 'changeset', $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(commit.repository_id)
        .bind(changeset.id)
        .bind(attachment.attachment_type)
        .bind(attachment.payload)
        .execute(&mut *tx)
        .await?;
        let conflict = sqlx::query_as::<_, ConflictRow>(
            "UPDATE conflicts
             SET state = 'resolved', resolution_changeset_id = $2,
                 acknowledged_drop = $4, resolved_by = $3, resolved_at = now()
             WHERE id = $1
             RETURNING id, repository_id, kind, state,
                       source_branch_id, target_branch_id,
                       source_branch_name, target_branch_name,
                       slice_id, source_changeset_id,
                       base_position, target_position,
                       conflicts, proposed_operations,
                       resolution_changeset_id, acknowledged_drop,
                       created_by, resolved_by, created_at, resolved_at",
        )
        .bind(conflict_id)
        .bind(changeset.id)
        .bind(resolved_by)
        .bind(acknowledged_drop)
        .fetch_one(&mut *tx)
        .await?;
        let conflict = conflict.try_into()?;
        tx.commit().await?;
        Ok(CommittedConflictResolution {
            conflict,
            changeset,
            operations,
        })
    }

    async fn enqueue_agent_job(
        &self,
        repository_id: Uuid,
        spec: AgentJobSpec,
        request_fingerprint: String,
        now: DateTime<Utc>,
    ) -> Result<AgentJob> {
        let available_at = spec.available_at.unwrap_or(now);
        let idempotency_key = spec.idempotency_key.clone();
        let tenant_key = spec.tenant_key.clone();
        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            INSERT INTO agent_jobs (
                id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                max_attempts, retry_backoff_seconds, max_backoff_seconds,
                available_at, request_fingerprint, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'queued', $7, $8, $9, $10, $11, $12, $12)
            ON CONFLICT (tenant_key, idempotency_key) DO UPDATE
            SET idempotency_key = agent_jobs.idempotency_key
            WHERE agent_jobs.request_fingerprint = EXCLUDED.request_fingerprint
            RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                      attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                      available_at, lease_owner, lease_expires_at, result, error,
                      created_at, updated_at, completed_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(spec.tenant_key)
        .bind(repository_id)
        .bind(spec.kind)
        .bind(spec.idempotency_key)
        .bind(spec.payload)
        .bind(spec.max_attempts)
        .bind(spec.retry_backoff_seconds)
        .bind(spec.max_backoff_seconds)
        .bind(available_at)
        .bind(request_fingerprint)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT request_fingerprint FROM agent_jobs
                 WHERE tenant_key = $1 AND idempotency_key = $2",
            )
            .bind(tenant_key)
            .bind(&idempotency_key)
            .fetch_optional(&self.pool)
            .await?;
            if existing.as_deref() == Some(&"0".repeat(64)) {
                bail!(
                    "idempotency key {idempotency_key} is already used by a different job request; \
                     the legacy job has no immutable request fingerprint, so use a new idempotency key"
                );
            }
            bail!("idempotency key {idempotency_key} is already used by a different job request");
        };
        row.try_into()
    }

    async fn agent_job_by_id(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
    ) -> Result<AgentJob> {
        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            SELECT id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                   attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                   available_at, lease_owner, lease_expires_at, result, error,
                   created_at, updated_at, completed_at
            FROM agent_jobs
            WHERE tenant_key = $1 AND repository_id = $2 AND id = $3
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(job_id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| {
            format!("agent job {job_id} does not exist in repository {repository_id}")
        })?;
        row.try_into()
    }

    async fn list_agent_jobs(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
    ) -> Result<Vec<AgentJob>> {
        let rows = sqlx::query_as::<_, AgentJobRow>(
            r#"
            SELECT id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                   attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                   available_at, lease_owner, lease_expires_at, result, error,
                   created_at, updated_at, completed_at
            FROM agent_jobs
            WHERE tenant_key = $1 AND repository_id = $2
            ORDER BY created_at, id
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn lease_agent_job(&self, request: LeaseAgentJobRequest<'_>) -> Result<Option<AgentJob>> {
        let LeaseAgentJobRequest {
            tenant_key,
            repository_id,
            owner,
            lease_token_digest,
            kind,
            now,
            lease_expires_at,
        } = request;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE agent_jobs
            SET state = 'failed',
                lease_owner = NULL,
                lease_token_digest = NULL,
                lease_expires_at = NULL,
                error = COALESCE(error, 'lease expired after maximum attempts'),
                completed_at = $3,
                updated_at = $3
            WHERE tenant_key = $1 AND repository_id = $2
              AND state = 'leased'
              AND lease_expires_at <= $3
              AND attempt_count >= max_attempts
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            WITH candidate AS (
                SELECT id
                FROM agent_jobs
                WHERE tenant_key = $1 AND repository_id = $2
                  AND ($5::text IS NULL OR kind = $5)
                  AND attempt_count < max_attempts
                  AND (
                    (state = 'queued' AND available_at <= $6)
                    OR (state = 'leased' AND lease_expires_at <= $6)
                  )
                ORDER BY available_at, created_at, id
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            UPDATE agent_jobs AS jobs
            SET state = 'leased',
                attempt_count = jobs.attempt_count + 1,
                lease_owner = $3,
                lease_token_digest = $4,
                lease_expires_at = $7,
                error = NULL,
                updated_at = $6
            FROM candidate
            WHERE jobs.id = candidate.id
            RETURNING jobs.id, jobs.tenant_key, jobs.repository_id, jobs.kind, jobs.idempotency_key,
                      jobs.payload, jobs.state, jobs.attempt_count, jobs.max_attempts,
                      jobs.retry_backoff_seconds, jobs.max_backoff_seconds, jobs.available_at,
                      jobs.lease_owner, jobs.lease_expires_at, jobs.result, jobs.error,
                      jobs.created_at, jobs.updated_at, jobs.completed_at
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(owner)
        .bind(lease_token_digest)
        .bind(kind)
        .bind(now)
        .bind(lease_expires_at)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        row.map(TryInto::try_into).transpose()
    }

    async fn heartbeat_agent_job(
        &self,
        lease: AgentJobLease<'_>,
        lease_expires_at: DateTime<Utc>,
    ) -> Result<AgentJob> {
        let AgentJobLease {
            tenant_key,
            repository_id,
            job_id,
            owner,
            lease_token_digest,
            now,
        } = lease;
        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            UPDATE agent_jobs
            SET lease_expires_at = $7, updated_at = $6
            WHERE tenant_key = $1 AND repository_id = $2 AND id = $3
              AND state = 'leased' AND lease_owner = $4
              AND lease_token_digest = $5 AND lease_expires_at > $6
            RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                      attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                      available_at, lease_owner, lease_expires_at, result, error,
                      created_at, updated_at, completed_at
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(job_id)
        .bind(owner)
        .bind(lease_token_digest)
        .bind(now)
        .bind(lease_expires_at)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("agent job {job_id} has no active lease owned by {owner}"))?;
        row.try_into()
    }

    async fn succeed_agent_job(&self, lease: AgentJobLease<'_>, result: Value) -> Result<AgentJob> {
        let AgentJobLease {
            tenant_key,
            repository_id,
            job_id,
            owner,
            lease_token_digest,
            now,
        } = lease;
        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            UPDATE agent_jobs
            SET state = 'succeeded', result = $7, error = NULL,
                lease_owner = NULL, lease_token_digest = NULL, lease_expires_at = NULL,
                completed_at = $6, updated_at = $6
            WHERE tenant_key = $1 AND repository_id = $2 AND id = $3
              AND state = 'leased' AND lease_owner = $4
              AND lease_token_digest = $5 AND lease_expires_at > $6
            RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                      attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                      available_at, lease_owner, lease_expires_at, result, error,
                      created_at, updated_at, completed_at
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(job_id)
        .bind(owner)
        .bind(lease_token_digest)
        .bind(now)
        .bind(result)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("agent job {job_id} has no active lease owned by {owner}"))?;
        row.try_into()
    }

    async fn fail_agent_job(&self, lease: AgentJobLease<'_>, error: &str) -> Result<AgentJob> {
        let AgentJobLease {
            tenant_key,
            repository_id,
            job_id,
            owner,
            lease_token_digest,
            now,
        } = lease;
        let mut tx = self.pool.begin().await?;
        let current: AgentJob = sqlx::query_as::<_, AgentJobRow>(
            r#"
            SELECT id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                   attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                   available_at, lease_owner, lease_expires_at, result, error,
                   created_at, updated_at, completed_at
            FROM agent_jobs
            WHERE tenant_key = $1 AND repository_id = $2 AND id = $3
              AND state = 'leased' AND lease_owner = $4
              AND lease_token_digest = $5 AND lease_expires_at > $6
            FOR UPDATE
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(job_id)
        .bind(owner)
        .bind(lease_token_digest)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .with_context(|| format!("agent job {job_id} has no active lease owned by {owner}"))?
        .try_into()?;
        let terminal = current.attempt_count >= current.max_attempts;
        let available_at = now + chrono::Duration::seconds(current.retry_delay_seconds());
        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            UPDATE agent_jobs
            SET state = CASE WHEN $4 THEN 'failed' ELSE 'queued' END,
                available_at = CASE WHEN $4 THEN available_at ELSE $5 END,
                lease_owner = NULL,
                lease_token_digest = NULL,
                lease_expires_at = NULL,
                result = NULL,
                error = $6,
                completed_at = CASE WHEN $4 THEN $7 ELSE NULL END,
                updated_at = $7
            WHERE tenant_key = $1 AND repository_id = $2 AND id = $3
            RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                      attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                      available_at, lease_owner, lease_expires_at, result, error,
                      created_at, updated_at, completed_at
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(job_id)
        .bind(terminal)
        .bind(available_at)
        .bind(error)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        row.try_into()
    }

    async fn cancel_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AgentJob> {
        let row = sqlx::query_as::<_, AgentJobRow>(
            r#"
            UPDATE agent_jobs
            SET state = 'cancelled', lease_owner = NULL, lease_token_digest = NULL,
                lease_expires_at = NULL,
                completed_at = COALESCE(completed_at, $4), updated_at = $4
            WHERE tenant_key = $1 AND repository_id = $2 AND id = $3
              AND state IN ('queued', 'leased', 'cancelled')
            RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                      attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                      available_at, lease_owner, lease_expires_at, result, error,
                      created_at, updated_at, completed_at
            "#,
        )
        .bind(tenant_key)
        .bind(repository_id)
        .bind(job_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("agent job {job_id} cannot be cancelled"))?;
        row.try_into()
    }

    async fn add_attachment(
        &self,
        repository_id: Uuid,
        entity_type: &str,
        entity_id: Uuid,
        attachment_type: &str,
        payload: Value,
    ) -> Result<Attachment> {
        let row = sqlx::query_as::<_, AttachmentRow>(
            r#"
            INSERT INTO attachments (id, repository_id, entity_type, entity_id, attachment_type, payload)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, repository_id, entity_type, entity_id, attachment_type, payload, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(repository_id)
        .bind(entity_type)
        .bind(entity_id)
        .bind(attachment_type)
        .bind(payload)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    async fn latest_graph_checkpoint(
        &self,
        branch_id: Uuid,
    ) -> Result<Option<StoredGraphCheckpoint>> {
        let row = sqlx::query_as::<_, GraphCacheRow>(
            r#"
            SELECT operation_count, operation_fingerprint, graph FROM graph_cache
            WHERE branch_id = $1
            ORDER BY operation_count DESC
            LIMIT 1
            "#,
        )
        .bind(branch_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| StoredGraphCheckpoint {
            operation_count: row.operation_count,
            operation_fingerprint: row.operation_fingerprint,
            graph: row.graph,
        }))
    }

    async fn store_graph_cache(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        operation_count: i64,
        fingerprint: &str,
        graph: Value,
        rendered_files: Value,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO graph_cache (branch_id, repository_id, operation_count, operation_fingerprint, graph, rendered_files)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (branch_id) DO UPDATE
            SET operation_count = EXCLUDED.operation_count,
                operation_fingerprint = EXCLUDED.operation_fingerprint,
                graph = EXCLUDED.graph,
                rendered_files = EXCLUDED.rendered_files,
                updated_at = now()
            "#,
        )
        .bind(branch_id)
        .bind(repository_id)
        .bind(operation_count)
        .bind(fingerprint)
        .bind(graph)
        .bind(rendered_files)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_source_file_snapshots(&self, branch_id: Uuid) -> Result<Vec<SourceFileSnapshot>> {
        let rows = sqlx::query_as::<_, SourceFileSnapshotRow>(
            r#"
            SELECT repository_id, branch_id, path, content_hash, byte_len, handler,
                   file_symbol_id, last_import_position, importer_version, updated_at
            FROM source_file_snapshots
            WHERE branch_id = $1
            ORDER BY path
            "#,
        )
        .bind(branch_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn replace_source_file_snapshots(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        snapshots: Vec<PendingSourceFileSnapshot>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM source_file_snapshots WHERE branch_id = $1")
            .bind(branch_id)
            .execute(&mut *tx)
            .await?;
        for snapshot in snapshots {
            sqlx::query(
                r#"
                INSERT INTO source_file_snapshots
                    (repository_id, branch_id, path, content_hash, byte_len, handler,
                     file_symbol_id, last_import_position, importer_version)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                "#,
            )
            .bind(repository_id)
            .bind(branch_id)
            .bind(snapshot.path)
            .bind(snapshot.content_hash)
            .bind(snapshot.byte_len)
            .bind(snapshot.handler)
            .bind(snapshot.file_symbol_id)
            .bind(snapshot.last_import_position)
            .bind(snapshot.importer_version)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
