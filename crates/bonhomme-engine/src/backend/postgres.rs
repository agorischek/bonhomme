//! Postgres backend (sqlx). The original engine SQL, moved verbatim behind `StorageBackend`.
use anyhow::{Context, Result, bail};
use bonhomme_core::{Branch, ChangeSet, OperationRecord, Repository, Task};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{PendingOperation, StorageBackend};
use crate::{
    Attachment, PendingSourceFileSnapshot, SourceFileSnapshot, StoredSlice,
    rows::{
        AttachmentRow, BranchRow, ChangeSetRow, GraphCacheRow, OperationRow, RepositoryRow,
        SliceRow, SourceFileSnapshotRow, TaskRow,
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

    async fn delete_repository_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM repositories WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
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

    async fn get_graph_cache(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        fingerprint: &str,
    ) -> Result<Option<(Value, Value)>> {
        let row = sqlx::query_as::<_, GraphCacheRow>(
            r#"
            SELECT graph, rendered_files FROM graph_cache
            WHERE branch_id = $1 AND operation_count = $2 AND operation_fingerprint = $3
            "#,
        )
        .bind(branch_id)
        .bind(operation_count)
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| (row.graph, row.rendered_files)))
    }

    async fn get_graph_cache_graph(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        fingerprint: &str,
    ) -> Result<Option<Value>> {
        let row = sqlx::query_scalar::<_, Value>(
            r#"
            SELECT graph FROM graph_cache
            WHERE branch_id = $1 AND operation_count = $2 AND operation_fingerprint = $3
            "#,
        )
        .bind(branch_id)
        .bind(operation_count)
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
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
