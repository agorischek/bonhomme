mod materialization;
mod merge;
mod models;
mod rows;

use anyhow::{Context, Result};
use bonhomme_core::{
    Branch, ChangeSet, LanguagePlugin, Operation, OperationRecord, Repository, Task,
};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{future::Future, pin::Pin, sync::Arc};
use uuid::Uuid;

pub use models::{Attachment, CacheStatus, MaterializedBranch, MergeResult};
use rows::{AttachmentRow, BranchRow, ChangeSetRow, OperationRow, RepositoryRow, TaskRow};

pub const DEFAULT_DATABASE_URL: &str = "postgres://bonhomme:bonhomme@localhost:54329/bonhomme";

/// The storage / merge engine. It is language-agnostic: every render, validate, import, and diff
/// goes through the injected [`LanguagePlugin`], so this module never depends on `ts`.
#[derive(Clone)]
pub struct Storage {
    pub(crate) pool: PgPool,
    pub(crate) plugin: Arc<dyn LanguagePlugin>,
}

impl Storage {
    pub async fn connect(database_url: &str, plugin: Arc<dyn LanguagePlugin>) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(12)
            .connect(database_url)
            .await
            .with_context(|| format!("failed to connect to Postgres at {database_url}"))?;
        Ok(Self { pool, plugin })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context("failed to run database migrations")?;
        Ok(())
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
        sqlx::query("DELETE FROM repositories WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        self.init_repository(name).await
    }

    pub async fn create_repository(&self, name: &str) -> Result<Repository> {
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, RepositoryRow>(
            r#"
            INSERT INTO repositories (id, name)
            VALUES ($1, $2)
            ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
            RETURNING id, name, created_at
            "#,
        )
        .bind(id)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    pub async fn repository_by_name(&self, name: &str) -> Result<Repository> {
        let row = sqlx::query_as::<_, RepositoryRow>(
            "SELECT id, name, created_at FROM repositories WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("repository {name} does not exist"))?;
        Ok(row.into())
    }

    pub async fn ensure_main_branch(&self, repository_id: Uuid) -> Result<Branch> {
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
            VALUES ($1, $2, 'main', NULL, 0)
            ON CONFLICT (repository_id, name) DO UPDATE SET name = EXCLUDED.name
            RETURNING id, repository_id, name, base_branch_id, base_position, created_at
            "#,
        )
        .bind(id)
        .bind(repository_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    pub async fn create_branch(
        &self,
        repository_id: Uuid,
        name: &str,
        base_name: &str,
    ) -> Result<Branch> {
        let base = self.branch_by_name(repository_id, base_name).await?;
        let base_position = self.collect_branch_operations(base.id, None).await?.len() as i64;
        let id = Uuid::new_v4();
        // The fork point (base_position) is immutable. If the branch already exists we return it
        // unchanged rather than advancing base_position to the current base length, which would
        // silently drop concurrent target operations from later merge analysis.
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (repository_id, name) DO NOTHING
            RETURNING id, repository_id, name, base_branch_id, base_position, created_at
            "#,
        )
        .bind(id)
        .bind(repository_id)
        .bind(name)
        .bind(base.id)
        .bind(base_position)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(row.into()),
            None => self.branch_by_name(repository_id, name).await,
        }
    }

    pub async fn branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<Branch> {
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            SELECT id, repository_id, name, base_branch_id, base_position, created_at
            FROM branches
            WHERE repository_id = $1 AND name = $2
            "#,
        )
        .bind(repository_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("branch {name} does not exist"))?;
        Ok(row.into())
    }

    pub async fn branch_by_id(&self, branch_id: Uuid) -> Result<Branch> {
        let row = sqlx::query_as::<_, BranchRow>(
            r#"
            SELECT id, repository_id, name, base_branch_id, base_position, created_at
            FROM branches
            WHERE id = $1
            "#,
        )
        .bind(branch_id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("branch {branch_id} does not exist"))?;
        Ok(row.into())
    }

    pub async fn list_branches(&self, repository_id: Uuid) -> Result<Vec<Branch>> {
        let rows = sqlx::query_as::<_, BranchRow>(
            r#"
            SELECT id, repository_id, name, base_branch_id, base_position, created_at
            FROM branches
            WHERE repository_id = $1
            ORDER BY created_at, name
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn create_task(&self, repository_id: Uuid, title: &str) -> Result<Task> {
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

    pub async fn create_changeset(
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

    pub async fn append_operation(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        operation: Operation,
    ) -> Result<OperationRecord> {
        let payload = serde_json::to_value(&operation)?;
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
        .bind(operation.op_type())
        .bind(payload)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        row.try_into()
    }

    pub async fn list_changesets(&self, repository_id: Uuid) -> Result<Vec<ChangeSet>> {
        let rows = sqlx::query_as::<_, ChangeSetRow>(
            r#"
            SELECT id, repository_id, task_id, branch_id, title, created_by, created_at
            FROM changesets
            WHERE repository_id = $1
            ORDER BY created_at, id
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_tasks(&self, repository_id: Uuid) -> Result<Vec<Task>> {
        let rows = sqlx::query_as::<_, TaskRow>(
            r#"
            SELECT id, repository_id, title, created_at
            FROM tasks
            WHERE repository_id = $1
            ORDER BY created_at, id
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_operations(&self, repository_id: Uuid) -> Result<Vec<OperationRecord>> {
        let rows = sqlx::query_as::<_, OperationRow>(
            r#"
            SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
            FROM operations
            WHERE repository_id = $1
            ORDER BY created_at, branch_id, position
            "#,
        )
        .bind(repository_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn list_own_operations(
        &self,
        branch_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<OperationRecord>> {
        let rows = if let Some(limit) = limit {
            sqlx::query_as::<_, OperationRow>(
                r#"
                SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                FROM operations
                WHERE branch_id = $1
                ORDER BY position
                LIMIT $2
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
                FROM operations
                WHERE branch_id = $1
                ORDER BY position
                "#,
            )
            .bind(branch_id)
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter().map(TryInto::try_into).collect()
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<OperationRecord>>> + Send + '_>> {
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
}
