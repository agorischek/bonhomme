//! Turso backend — the experimental in-process Rust SQLite rewrite. UUIDs and JSON are stored as
//! TEXT, timestamps as `CURRENT_TIMESTAMP` strings, and per-branch position allocation is a single
//! atomic `INSERT … (SELECT MAX(position)+1 …)` (the single-writer model removes the read/write
//! race the Postgres backend needs an advisory lock for).
use anyhow::{Context, Result, bail};
use bonhomme_core::{Branch, ChangeSet, OperationRecord, Repository, Task};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use turso::{Builder, Connection, Database, Row, Value as TursoValue, params};
use uuid::Uuid;

use super::StorageBackend;
use crate::{Attachment, StoredSlice};

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS repositories (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS branches (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        base_branch_id TEXT REFERENCES branches(id) ON DELETE SET NULL,
        base_position INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE (repository_id, name)
    )",
    "CREATE TABLE IF NOT EXISTS tasks (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        title TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS changesets (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
        title TEXT NOT NULL,
        created_by TEXT NOT NULL DEFAULT 'human',
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS operations (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
        changeset_id TEXT NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        op_type TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE (branch_id, position)
    )",
    "CREATE TABLE IF NOT EXISTS attachments (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        entity_type TEXT NOT NULL,
        entity_id TEXT NOT NULL,
        attachment_type TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS graph_cache (
        branch_id TEXT PRIMARY KEY REFERENCES branches(id) ON DELETE CASCADE,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        operation_count INTEGER NOT NULL,
        operation_fingerprint TEXT NOT NULL,
        graph TEXT NOT NULL,
        rendered_files TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS slices (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
        base_position INTEGER NOT NULL,
        root_symbols TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE INDEX IF NOT EXISTS idx_operations_branch_position ON operations(branch_id, position)",
    "CREATE INDEX IF NOT EXISTS idx_operations_repository ON operations(repository_id)",
];

pub(crate) struct TursoBackend {
    db: Database,
}

impl TursoBackend {
    pub(crate) async fn connect(path: &str) -> Result<Self> {
        let db = Builder::new_local(path)
            .build()
            .await
            .with_context(|| format!("failed to open Turso database at {path}"))?;
        Ok(Self { db })
    }

    async fn conn(&self) -> Result<Connection> {
        let conn = self.db.connect()?;
        // Best-effort: the engine also deletes children explicitly, so cascade enforcement is not
        // load-bearing if the beta engine ignores this pragma.
        let _ = conn.execute("PRAGMA foreign_keys = ON", ()).await;
        Ok(conn)
    }
}

#[async_trait::async_trait]
impl StorageBackend for TursoBackend {
    async fn run_migrations(&self) -> Result<()> {
        let conn = self.conn().await?;
        for statement in SCHEMA {
            conn.execute(statement, ())
                .await
                .with_context(|| format!("turso schema statement failed: {statement}"))?;
        }
        Ok(())
    }

    async fn delete_repository_by_name(&self, name: &str) -> Result<()> {
        let conn = self.conn().await?;
        let id = {
            let mut rows = conn
                .query("SELECT id FROM repositories WHERE name = ?1", params![name])
                .await?;
            match rows.next().await? {
                Some(row) => text(&row, 0)?,
                None => return Ok(()),
            }
        };
        for table in [
            "graph_cache",
            "slices",
            "attachments",
            "operations",
            "changesets",
            "tasks",
            "branches",
        ] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE repository_id = ?1"),
                params![id.clone()],
            )
            .await?;
        }
        conn.execute("DELETE FROM repositories WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    async fn create_repository(&self, name: &str) -> Result<Repository> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO repositories (id, name) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET name = excluded.name
                 RETURNING id, name, created_at",
                params![Uuid::new_v4().to_string(), name],
            )
            .await?;
        repository(&one(&mut rows).await?)
    }

    async fn repository_by_name(&self, name: &str) -> Result<Repository> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, name, created_at FROM repositories WHERE name = ?1",
                params![name],
            )
            .await?;
        repository(&rows.next().await?.with_context(|| format!("repository {name} does not exist"))?)
    }

    async fn repository_by_id(&self, id: Uuid) -> Result<Repository> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, name, created_at FROM repositories WHERE id = ?1",
                params![id.to_string()],
            )
            .await?;
        repository(&rows.next().await?.with_context(|| format!("repository {id} does not exist"))?)
    }

    async fn ensure_main_branch(&self, repository_id: Uuid) -> Result<Branch> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
                 VALUES (?1, ?2, 'main', NULL, 0)
                 ON CONFLICT(repository_id, name) DO UPDATE SET name = excluded.name
                 RETURNING id, repository_id, name, base_branch_id, base_position, created_at",
                params![Uuid::new_v4().to_string(), repository_id.to_string()],
            )
            .await?;
        branch(&one(&mut rows).await?)
    }

    async fn insert_branch(
        &self,
        repository_id: Uuid,
        name: &str,
        base_branch_id: Uuid,
        base_position: i64,
    ) -> Result<Option<Branch>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(repository_id, name) DO NOTHING
                 RETURNING id, repository_id, name, base_branch_id, base_position, created_at",
                params![
                    Uuid::new_v4().to_string(),
                    repository_id.to_string(),
                    name,
                    base_branch_id.to_string(),
                    base_position
                ],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(branch(&row)?)),
            None => Ok(None),
        }
    }

    async fn branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<Branch> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, name, base_branch_id, base_position, created_at
                 FROM branches WHERE repository_id = ?1 AND name = ?2",
                params![repository_id.to_string(), name],
            )
            .await?;
        branch(&rows.next().await?.with_context(|| format!("branch {name} does not exist"))?)
    }

    async fn branch_by_id(&self, branch_id: Uuid) -> Result<Branch> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, name, base_branch_id, base_position, created_at
                 FROM branches WHERE id = ?1",
                params![branch_id.to_string()],
            )
            .await?;
        branch(&rows.next().await?.with_context(|| format!("branch {branch_id} does not exist"))?)
    }

    async fn list_branches(&self, repository_id: Uuid) -> Result<Vec<Branch>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, name, base_branch_id, base_position, created_at
                 FROM branches WHERE repository_id = ?1 ORDER BY created_at, name",
                params![repository_id.to_string()],
            )
            .await?;
        collect(&mut rows, branch).await
    }

    async fn create_task(&self, repository_id: Uuid, title: &str) -> Result<Task> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO tasks (id, repository_id, title) VALUES (?1, ?2, ?3)
                 RETURNING id, repository_id, title, created_at",
                params![Uuid::new_v4().to_string(), repository_id.to_string(), title],
            )
            .await?;
        task(&one(&mut rows).await?)
    }

    async fn list_tasks(&self, repository_id: Uuid) -> Result<Vec<Task>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, title, created_at FROM tasks
                 WHERE repository_id = ?1 ORDER BY created_at, id",
                params![repository_id.to_string()],
            )
            .await?;
        collect(&mut rows, task).await
    }

    async fn create_changeset(
        &self,
        repository_id: Uuid,
        task_id: Uuid,
        branch_id: Uuid,
        title: &str,
        created_by: &str,
    ) -> Result<ChangeSet> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO changesets (id, repository_id, task_id, branch_id, title, created_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING id, repository_id, task_id, branch_id, title, created_by, created_at",
                params![
                    Uuid::new_v4().to_string(),
                    repository_id.to_string(),
                    task_id.to_string(),
                    branch_id.to_string(),
                    title,
                    created_by
                ],
            )
            .await?;
        changeset(&one(&mut rows).await?)
    }

    async fn list_changesets(&self, repository_id: Uuid) -> Result<Vec<ChangeSet>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, task_id, branch_id, title, created_by, created_at
                 FROM changesets WHERE repository_id = ?1 ORDER BY created_at, id",
                params![repository_id.to_string()],
            )
            .await?;
        collect(&mut rows, changeset).await
    }

    async fn append_operation(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        op_type: &str,
        payload: Value,
    ) -> Result<OperationRecord> {
        let conn = self.conn().await?;
        // Single-statement position allocation: the subquery runs inside the same write, and the
        // single-writer model serializes writers, so two appends cannot collide on (branch, pos).
        let mut rows = conn
            .query(
                "INSERT INTO operations (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                 VALUES (?1, ?2, ?3, ?4,
                         (SELECT COALESCE(MAX(position), 0) + 1 FROM operations WHERE branch_id = ?3),
                         ?5, ?6)
                 RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at",
                params![
                    Uuid::new_v4().to_string(),
                    repository_id.to_string(),
                    branch_id.to_string(),
                    changeset_id.to_string(),
                    op_type,
                    serde_json::to_string(&payload)?
                ],
            )
            .await?;
        operation(&one(&mut rows).await?)
    }

    async fn list_operations(&self, repository_id: Uuid) -> Result<Vec<OperationRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                 FROM operations WHERE repository_id = ?1 ORDER BY created_at, branch_id, position",
                params![repository_id.to_string()],
            )
            .await?;
        collect(&mut rows, operation).await
    }

    async fn list_own_operations(
        &self,
        branch_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<OperationRecord>> {
        let conn = self.conn().await?;
        let mut rows = if let Some(limit) = limit {
            conn.query(
                "SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                 FROM operations WHERE branch_id = ?1 ORDER BY position LIMIT ?2",
                params![branch_id.to_string(), limit],
            )
            .await?
        } else {
            conn.query(
                "SELECT id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at
                 FROM operations WHERE branch_id = ?1 ORDER BY position",
                params![branch_id.to_string()],
            )
            .await?
        };
        collect(&mut rows, operation).await
    }

    async fn insert_slice(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        base_position: i64,
        root_symbols: Value,
    ) -> Result<StoredSlice> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO slices (id, repository_id, branch_id, base_position, root_symbols)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 RETURNING id, repository_id, branch_id, base_position, root_symbols, created_at",
                params![
                    Uuid::new_v4().to_string(),
                    repository_id.to_string(),
                    branch_id.to_string(),
                    base_position,
                    serde_json::to_string(&root_symbols)?
                ],
            )
            .await?;
        slice(&one(&mut rows).await?)
    }

    async fn slice_by_id(&self, slice_id: Uuid) -> Result<StoredSlice> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, branch_id, base_position, root_symbols, created_at
                 FROM slices WHERE id = ?1",
                params![slice_id.to_string()],
            )
            .await?;
        slice(&rows.next().await?.with_context(|| format!("slice {slice_id} does not exist"))?)
    }

    async fn add_attachment(
        &self,
        repository_id: Uuid,
        entity_type: &str,
        entity_id: Uuid,
        attachment_type: &str,
        payload: Value,
    ) -> Result<Attachment> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "INSERT INTO attachments (id, repository_id, entity_type, entity_id, attachment_type, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 RETURNING id, repository_id, entity_type, entity_id, attachment_type, payload, created_at",
                params![
                    Uuid::new_v4().to_string(),
                    repository_id.to_string(),
                    entity_type,
                    entity_id.to_string(),
                    attachment_type,
                    serde_json::to_string(&payload)?
                ],
            )
            .await?;
        attachment(&one(&mut rows).await?)
    }

    async fn get_graph_cache(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        fingerprint: &str,
    ) -> Result<Option<(Value, Value)>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT graph, rendered_files FROM graph_cache
                 WHERE branch_id = ?1 AND operation_count = ?2 AND operation_fingerprint = ?3",
                params![branch_id.to_string(), operation_count, fingerprint],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some((json(&row, 0)?, json(&row, 1)?))),
            None => Ok(None),
        }
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
        let conn = self.conn().await?;
        conn.execute(
            "INSERT INTO graph_cache (branch_id, repository_id, operation_count, operation_fingerprint, graph, rendered_files)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(branch_id) DO UPDATE SET
                 operation_count = excluded.operation_count,
                 operation_fingerprint = excluded.operation_fingerprint,
                 graph = excluded.graph,
                 rendered_files = excluded.rendered_files,
                 updated_at = CURRENT_TIMESTAMP",
            params![
                branch_id.to_string(),
                repository_id.to_string(),
                operation_count,
                fingerprint,
                serde_json::to_string(&graph)?,
                serde_json::to_string(&rendered_files)?
            ],
        )
        .await?;
        Ok(())
    }
}

// ---- row mapping ----

async fn one(rows: &mut turso::Rows) -> Result<Row> {
    rows.next().await?.context("expected a returned row")
}

async fn collect<T>(rows: &mut turso::Rows, map: fn(&Row) -> Result<T>) -> Result<Vec<T>> {
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(map(&row)?);
    }
    Ok(out)
}

fn text(row: &Row, i: usize) -> Result<String> {
    match row.get_value(i)? {
        TursoValue::Text(value) => Ok(value),
        other => bail!("column {i}: expected TEXT, got {other:?}"),
    }
}

fn text_opt(row: &Row, i: usize) -> Result<Option<String>> {
    match row.get_value(i)? {
        TursoValue::Null => Ok(None),
        TursoValue::Text(value) => Ok(Some(value)),
        other => bail!("column {i}: expected TEXT or NULL, got {other:?}"),
    }
}

fn int(row: &Row, i: usize) -> Result<i64> {
    match row.get_value(i)? {
        TursoValue::Integer(value) => Ok(value),
        other => bail!("column {i}: expected INTEGER, got {other:?}"),
    }
}

fn uuid(row: &Row, i: usize) -> Result<Uuid> {
    Uuid::parse_str(&text(row, i)?).map_err(Into::into)
}

fn uuid_opt(row: &Row, i: usize) -> Result<Option<Uuid>> {
    match text_opt(row, i)? {
        Some(value) => Ok(Some(Uuid::parse_str(&value)?)),
        None => Ok(None),
    }
}

fn json(row: &Row, i: usize) -> Result<Value> {
    Ok(serde_json::from_str(&text(row, i)?)?)
}

fn timestamp(row: &Row, i: usize) -> Result<DateTime<Utc>> {
    let raw = text(row, i)?;
    let naive = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&raw, "%Y-%m-%dT%H:%M:%S%.f"))
        .with_context(|| format!("unparseable timestamp {raw:?}"))?;
    Ok(naive.and_utc())
}

fn repository(row: &Row) -> Result<Repository> {
    Ok(Repository {
        id: uuid(row, 0)?,
        name: text(row, 1)?,
        created_at: timestamp(row, 2)?,
    })
}

fn branch(row: &Row) -> Result<Branch> {
    Ok(Branch {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        name: text(row, 2)?,
        base_branch_id: uuid_opt(row, 3)?,
        base_position: int(row, 4)?,
        created_at: timestamp(row, 5)?,
    })
}

fn task(row: &Row) -> Result<Task> {
    Ok(Task {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        title: text(row, 2)?,
        created_at: timestamp(row, 3)?,
    })
}

fn changeset(row: &Row) -> Result<ChangeSet> {
    Ok(ChangeSet {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        task_id: uuid(row, 2)?,
        branch_id: uuid(row, 3)?,
        title: text(row, 4)?,
        created_by: text(row, 5)?,
        created_at: timestamp(row, 6)?,
    })
}

fn operation(row: &Row) -> Result<OperationRecord> {
    Ok(OperationRecord {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        branch_id: uuid(row, 2)?,
        changeset_id: uuid(row, 3)?,
        position: int(row, 4)?,
        // column 5 is op_type (redundant with the payload's tag)
        operation: serde_json::from_value(json(row, 6)?)?,
        created_at: timestamp(row, 7)?,
    })
}

fn attachment(row: &Row) -> Result<Attachment> {
    Ok(Attachment {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        entity_type: text(row, 2)?,
        entity_id: uuid(row, 3)?,
        attachment_type: text(row, 4)?,
        payload: json(row, 5)?,
        created_at: timestamp(row, 6)?,
    })
}

fn slice(row: &Row) -> Result<StoredSlice> {
    Ok(StoredSlice {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        branch_id: uuid(row, 2)?,
        base_position: int(row, 3)?,
        root_symbols: serde_json::from_value(json(row, 4)?)?,
        created_at: timestamp(row, 5)?,
    })
}
