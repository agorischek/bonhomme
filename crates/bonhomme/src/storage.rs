use crate::core::{
    Branch, ChangeSet, MergeConflict, MergeOutcome, Operation, OperationRecord, Repository,
    SemanticGraph, Task, analyze_merge, materialize,
};
use crate::ts::{RenderedFile, render_files, validate_typescript_files};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};
use std::{future::Future, pin::Pin};
use uuid::Uuid;

pub const DEFAULT_DATABASE_URL: &str = "postgres://bonhomme:bonhomme@localhost:54329/bonhomme";

#[derive(Clone)]
pub struct Storage {
    pool: PgPool,
}

impl Storage {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(12)
            .connect(database_url)
            .await
            .with_context(|| format!("failed to connect to Postgres at {database_url}"))?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context("failed to run database migrations")?;
        Ok(())
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

    pub async fn materialize_branch(
        &self,
        repository_name: &str,
        branch_name: &str,
    ) -> Result<MaterializedBranch> {
        let repository = self.repository_by_name(repository_name).await?;
        let branch = self.branch_by_name(repository.id, branch_name).await?;
        let operations = self.collect_branch_operations(branch.id, None).await?;
        let operation_count = operations.len() as i64;
        let operation_fingerprint = operation_fingerprint(&operations);

        if let Some((graph, files)) = self
            .cached_materialization(branch.id, operation_count, &operation_fingerprint)
            .await?
        {
            return Ok(MaterializedBranch {
                repository,
                branch,
                operations,
                graph,
                files,
                cache_status: CacheStatus::Hit,
            });
        }

        let graph = materialize(&operations)?;
        let files = render_files(&graph);
        self.store_graph_cache(
            repository.id,
            branch.id,
            operation_count,
            &operation_fingerprint,
            &graph,
            &files,
        )
        .await?;
        Ok(MaterializedBranch {
            repository,
            branch,
            operations,
            graph,
            files,
            cache_status: CacheStatus::Miss,
        })
    }

    async fn cached_materialization(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        operation_fingerprint: &str,
    ) -> Result<Option<(SemanticGraph, Vec<RenderedFile>)>> {
        let Some(row) = sqlx::query_as::<_, GraphCacheRow>(
            r#"
            SELECT graph, rendered_files
            FROM graph_cache
            WHERE branch_id = $1 AND operation_count = $2 AND operation_fingerprint = $3
            "#,
        )
        .bind(branch_id)
        .bind(operation_count)
        .bind(operation_fingerprint)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let graph = serde_json::from_value(row.graph)?;
        let files = serde_json::from_value(row.rendered_files)?;
        Ok(Some((graph, files)))
    }

    async fn store_graph_cache(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        operation_count: i64,
        operation_fingerprint: &str,
        graph: &SemanticGraph,
        files: &[RenderedFile],
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
        .bind(operation_fingerprint)
        .bind(serde_json::to_value(graph)?)
        .bind(serde_json::to_value(files)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn merge_branch(
        &self,
        repository_name: &str,
        source_branch_name: &str,
        target_branch_name: &str,
    ) -> Result<MergeResult> {
        let repository = self.repository_by_name(repository_name).await?;
        let source = self
            .branch_by_name(repository.id, source_branch_name)
            .await?;
        let target = self
            .branch_by_name(repository.id, target_branch_name)
            .await?;

        if source.base_branch_id != Some(target.id) {
            bail!(
                "bonhomme v1 merge prototype only supports direct merges from a branch based on the target branch"
            );
        }

        let target_base_operations = self
            .collect_branch_operations(target.id, Some(source.base_position))
            .await?;
        let target_operations = self.collect_branch_operations(target.id, None).await?;
        let target_since_base = target_operations
            .iter()
            .skip(target_base_operations.len())
            .cloned()
            .collect::<Vec<_>>();
        let source_operations = self.list_own_operations(source.id, None).await?;

        let mut analysis = analyze_merge(&target_since_base, &source_operations);
        if analysis.outcome == MergeOutcome::SafeMerge {
            let mut target_graph = materialize(&target_operations)?;
            for operation in &source_operations {
                if let Err(error) = target_graph.apply_record(operation) {
                    analysis.outcome = MergeOutcome::Conflict;
                    analysis.conflicts.push(MergeConflict {
                        reason: "VALIDATION_REJECTED".to_string(),
                        source_operation_id: operation.id,
                        target_operation_id: None,
                        symbol_id: operation.operation.created_symbol_id(),
                        detail: error.to_string(),
                    });
                    break;
                }
            }
            if analysis.outcome == MergeOutcome::SafeMerge {
                let files = render_files(&target_graph);
                if let Err(error) = validate_typescript_files(&files).await {
                    analysis.outcome = MergeOutcome::Conflict;
                    analysis.conflicts.push(MergeConflict {
                        reason: "TSC_REJECTED".to_string(),
                        source_operation_id: source_operations
                            .first()
                            .map(|operation| operation.id)
                            .unwrap_or_else(Uuid::nil),
                        target_operation_id: None,
                        symbol_id: None,
                        detail: error.to_string(),
                    });
                }
            }
        }

        if analysis.outcome == MergeOutcome::Conflict {
            let graph = materialize(&target_operations)?;
            return Ok(MergeResult {
                outcome: MergeOutcome::Conflict,
                conflicts: analysis.conflicts,
                source_branch: source,
                target_branch: target,
                appended_operations: Vec::new(),
                target_position: target_operations.len() as i64,
                files: render_files(&graph),
            });
        }

        let task = self
            .create_task(
                repository.id,
                &format!("Merge {source_branch_name} into {target_branch_name}"),
            )
            .await?;
        let changeset = self
            .create_changeset(
                repository.id,
                task.id,
                target.id,
                &format!("Merge {source_branch_name}"),
                "merge-engine",
            )
            .await?;

        let mut appended = Vec::new();
        for source_operation in source_operations {
            appended.push(
                self.append_operation(
                    repository.id,
                    target.id,
                    changeset.id,
                    source_operation.operation,
                )
                .await?,
            );
        }

        let updated_operations = self.collect_branch_operations(target.id, None).await?;
        let graph = materialize(&updated_operations)?;
        Ok(MergeResult {
            outcome: MergeOutcome::SafeMerge,
            conflicts: Vec::new(),
            source_branch: source,
            target_branch: target,
            appended_operations: appended,
            target_position: updated_operations.len() as i64,
            files: render_files(&graph),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializedBranch {
    pub repository: Repository,
    pub branch: Branch,
    pub operations: Vec<OperationRecord>,
    pub graph: SemanticGraph,
    pub files: Vec<RenderedFile>,
    pub cache_status: CacheStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CacheStatus {
    Hit,
    Miss,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub outcome: MergeOutcome,
    pub conflicts: Vec<MergeConflict>,
    pub source_branch: Branch,
    pub target_branch: Branch,
    pub appended_operations: Vec<OperationRecord>,
    pub target_position: i64,
    pub files: Vec<RenderedFile>,
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

#[derive(FromRow)]
struct RepositoryRow {
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
struct BranchRow {
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
struct TaskRow {
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
struct ChangeSetRow {
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
struct OperationRow {
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
struct AttachmentRow {
    id: Uuid,
    repository_id: Uuid,
    entity_type: String,
    entity_id: Uuid,
    attachment_type: String,
    payload: Value,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct GraphCacheRow {
    graph: Value,
    rendered_files: Value,
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

fn operation_fingerprint(operations: &[OperationRecord]) -> String {
    operations
        .iter()
        .map(|operation| format!("{}:{}", operation.id, operation.position))
        .collect::<Vec<_>>()
        .join("|")
}
