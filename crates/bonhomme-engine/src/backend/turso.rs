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

use super::{
    AgentJobLease, BranchAdvanced, CommittedConflictResolution, CommittedOperations,
    LeaseAgentJobRequest, PendingAttachment, PendingConflictResolutionCommit, PendingOperation,
    PendingOperationCommit, StorageBackend, StoredGraphCheckpoint,
};
use crate::{
    AgentJob, AgentJobSpec, Attachment, ConflictKind, ConflictRecord, ConflictState,
    PendingSourceFileSnapshot, SourceFileSnapshot, StoredSlice, models::PendingConflict,
};

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
    "CREATE TABLE IF NOT EXISTS source_file_snapshots (
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        byte_len INTEGER NOT NULL,
        handler TEXT NOT NULL,
        file_symbol_id TEXT,
        last_import_position INTEGER NOT NULL,
        importer_version TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (branch_id, path)
    )",
    "CREATE TABLE IF NOT EXISTS slices (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
        base_position INTEGER NOT NULL,
        root_symbols TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS conflicts (
        id TEXT PRIMARY KEY,
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        state TEXT NOT NULL,
        source_branch_id TEXT REFERENCES branches(id) ON DELETE SET NULL,
        target_branch_id TEXT REFERENCES branches(id) ON DELETE SET NULL,
        source_branch_name TEXT NOT NULL,
        target_branch_name TEXT NOT NULL,
        slice_id TEXT REFERENCES slices(id) ON DELETE SET NULL,
        source_changeset_id TEXT REFERENCES changesets(id) ON DELETE SET NULL,
        base_position INTEGER NOT NULL,
        target_position INTEGER NOT NULL,
        conflicts TEXT NOT NULL,
        proposed_operations TEXT NOT NULL,
        resolution_changeset_id TEXT REFERENCES changesets(id) ON DELETE SET NULL,
        acknowledged_drop INTEGER NOT NULL DEFAULT 0,
        created_by TEXT NOT NULL,
        resolved_by TEXT,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        resolved_at TEXT
    )",
    "CREATE TABLE IF NOT EXISTS agent_jobs (
        id TEXT PRIMARY KEY,
        tenant_key TEXT NOT NULL CHECK (length(CAST(tenant_key AS BLOB)) BETWEEN 1 AND 128),
        repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK (length(CAST(kind AS BLOB)) BETWEEN 1 AND 128),
        idempotency_key TEXT NOT NULL CHECK (length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 256),
        request_fingerprint TEXT NOT NULL CHECK (length(CAST(request_fingerprint AS BLOB)) = 64),
        payload TEXT NOT NULL,
        state TEXT NOT NULL DEFAULT 'queued'
            CHECK (state IN ('queued', 'leased', 'succeeded', 'failed', 'cancelled')),
        attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND max_attempts),
        max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
        retry_backoff_seconds INTEGER NOT NULL CHECK (retry_backoff_seconds BETWEEN 0 AND 86400),
        max_backoff_seconds INTEGER NOT NULL CHECK (max_backoff_seconds BETWEEN retry_backoff_seconds AND 604800),
        available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        lease_owner TEXT CHECK (lease_owner IS NULL OR length(CAST(lease_owner AS BLOB)) BETWEEN 1 AND 128),
        lease_token_digest TEXT CHECK (
            lease_token_digest IS NULL
            OR length(CAST(lease_token_digest AS BLOB)) = 64
        ),
        lease_expires_at TEXT,
        result TEXT,
        error TEXT CHECK (error IS NULL OR length(CAST(error AS BLOB)) <= 16384),
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        completed_at TEXT,
        CHECK (
            (state = 'leased' AND lease_owner IS NOT NULL AND lease_token_digest IS NOT NULL
                AND lease_expires_at IS NOT NULL)
            OR (state <> 'leased' AND lease_owner IS NULL AND lease_token_digest IS NULL
                AND lease_expires_at IS NULL)
        ),
        CHECK ((state IN ('succeeded', 'failed', 'cancelled')) = (completed_at IS NOT NULL)),
        CHECK (result IS NULL OR state = 'succeeded'),
        UNIQUE (tenant_key, idempotency_key)
    )",
    "CREATE INDEX IF NOT EXISTS idx_operations_branch_position ON operations(branch_id, position)",
    "CREATE INDEX IF NOT EXISTS idx_operations_repository ON operations(repository_id)",
    "CREATE INDEX IF NOT EXISTS idx_source_file_snapshots_repository ON source_file_snapshots(repository_id)",
    "CREATE INDEX IF NOT EXISTS idx_conflicts_repository_state ON conflicts(repository_id, state, created_at)",
    "CREATE INDEX IF NOT EXISTS idx_conflicts_source_branch ON conflicts(source_branch_id, state)",
    "CREATE INDEX IF NOT EXISTS idx_conflicts_target_branch ON conflicts(target_branch_id, state)",
    "CREATE INDEX IF NOT EXISTS idx_agent_jobs_available ON agent_jobs(tenant_key, repository_id, state, available_at, created_at)",
    "CREATE INDEX IF NOT EXISTS idx_agent_jobs_expired_leases ON agent_jobs(tenant_key, repository_id, lease_expires_at) WHERE state = 'leased'",
];

pub(crate) struct TursoBackend {
    db: Database,
}

impl TursoBackend {
    pub(crate) async fn connect(path: &str) -> Result<Self> {
        // Create the parent directory for an on-disk database so a default like
        // `turso:.bonhomme/bonhomme.db` works on a fresh checkout without manual setup.
        if path != ":memory:"
            && let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating database directory {}", parent.display()))?;
        }
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

    async fn append_operations_transactional(
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

        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let mut rows = conn
                .query(
                    "SELECT COALESCE(MAX(position), 0) FROM operations WHERE branch_id = ?1",
                    params![branch_id.to_string()],
                )
                .await?;
            let current_position = int(&one(&mut rows).await?, 0)?;
            if let Some(expected) = expected_current_position
                && current_position != expected
            {
                bail!(
                    "branch {branch_id} advanced from position {expected} to {current_position}; retry merge"
                );
            }

            let mut appended = Vec::with_capacity(operations.len());
            for (next_position, pending) in (current_position + 1..).zip(operations) {
                let mut rows = conn
                    .query(
                        "INSERT INTO operations (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at",
                        params![
                            Uuid::new_v4().to_string(),
                            repository_id.to_string(),
                            branch_id.to_string(),
                            changeset_id.to_string(),
                            next_position,
                            pending.op_type,
                            serde_json::to_string(&pending.payload)?
                        ],
                    )
                    .await?;
                appended.push(operation(&one(&mut rows).await?)?);
            }
            Ok(appended)
        }
        .await;

        match result {
            Ok(appended) => {
                conn.execute("COMMIT", ()).await?;
                Ok(appended)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl StorageBackend for TursoBackend {
    async fn run_migrations(&self) -> Result<()> {
        let conn = self.conn().await?;
        conn.execute("PRAGMA busy_timeout = 5000", ()).await?;
        // Serialize the complete schema and upgrade sequence. This covers both fresh startup and
        // two processes concurrently opening a database created before lease tokens existed.
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let upgrade = async {
            for statement in SCHEMA {
                conn.execute(statement, ())
                    .await
                    .with_context(|| format!("turso schema statement failed: {statement}"))?;
            }
            // `CREATE TABLE IF NOT EXISTS` cannot evolve an existing local database.
            let mut columns = conn.query("PRAGMA table_info(agent_jobs)", ()).await?;
            let mut has_lease_token_digest = false;
            let mut has_request_fingerprint = false;
            while let Some(row) = columns.next().await? {
                match text(&row, 1)?.as_str() {
                    "lease_token_digest" => has_lease_token_digest = true,
                    "request_fingerprint" => has_request_fingerprint = true,
                    _ => {}
                }
            }
            if !has_lease_token_digest {
                conn.execute(
                    "ALTER TABLE agent_jobs ADD COLUMN lease_token_digest TEXT
                     CHECK (
                         lease_token_digest IS NULL
                         OR length(CAST(lease_token_digest AS BLOB)) = 64
                     )",
                    (),
                )
                .await
                .context("upgrading agent_jobs with lease possession tokens")?;
            }
            if !has_request_fingerprint {
                conn.execute(
                    "ALTER TABLE agent_jobs ADD COLUMN request_fingerprint TEXT NOT NULL
                     DEFAULT '0000000000000000000000000000000000000000000000000000000000000000'
                     CHECK (length(CAST(request_fingerprint AS BLOB)) = 64)",
                    (),
                )
                .await
                .context("upgrading agent_jobs with immutable request fingerprints")?;
            }

            // Legacy tables cannot gain table-level CHECK constraints with ALTER TABLE. Reject
            // invalid legacy contents, then enforce the Postgres state/digest invariants with
            // triggers for all future writes. The triggers are harmlessly redundant on new DBs.
            let mut invalid = conn
                .query(
                    "SELECT id FROM agent_jobs
                     WHERE state NOT IN ('queued', 'leased', 'succeeded', 'failed', 'cancelled')
                        OR (lease_token_digest IS NOT NULL
                            AND length(CAST(lease_token_digest AS BLOB)) != 64)
                     LIMIT 1",
                    (),
                )
                .await?;
            if let Some(row) = invalid.next().await? {
                bail!(
                    "agent_jobs contains invalid legacy row {}; repair it before migration",
                    text(&row, 0)?
                );
            }
            for statement in [
                "CREATE TRIGGER IF NOT EXISTS validate_agent_jobs_insert
                 BEFORE INSERT ON agent_jobs FOR EACH ROW
                 WHEN length(CAST(NEW.request_fingerprint AS BLOB)) != 64
                    OR NEW.state NOT IN ('queued', 'leased', 'succeeded', 'failed', 'cancelled')
                    OR (NEW.lease_token_digest IS NOT NULL
                        AND length(CAST(NEW.lease_token_digest AS BLOB)) != 64)
                 BEGIN
                     SELECT RAISE(ABORT, 'invalid agent_jobs state or lease token digest');
                 END",
                "CREATE TRIGGER IF NOT EXISTS validate_agent_jobs_update
                 BEFORE UPDATE ON agent_jobs FOR EACH ROW
                 WHEN length(CAST(NEW.request_fingerprint AS BLOB)) != 64
                    OR NEW.state NOT IN ('queued', 'leased', 'succeeded', 'failed', 'cancelled')
                    OR (NEW.lease_token_digest IS NOT NULL
                        AND length(CAST(NEW.lease_token_digest AS BLOB)) != 64)
                 BEGIN
                     SELECT RAISE(ABORT, 'invalid agent_jobs state or lease token digest');
                 END",
            ] {
                conn.execute(statement, ()).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match upgrade {
            Ok(()) => {
                conn.execute("COMMIT", ()).await?;
                Ok(())
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn reset_repository_by_name(&self, name: &str) -> Result<(Repository, Branch)> {
        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let repository_id = {
                let mut rows = conn
                    .query("SELECT id FROM repositories WHERE name = ?1", params![name])
                    .await?;
                rows.next().await?.map(|row| text(&row, 0)).transpose()?
            };
            if let Some(repository_id) = repository_id {
                for table in [
                    "graph_cache",
                    "source_file_snapshots",
                    "conflicts",
                    "agent_jobs",
                    "slices",
                    "attachments",
                    "operations",
                    "changesets",
                    "tasks",
                    "branches",
                ] {
                    conn.execute(
                        &format!("DELETE FROM {table} WHERE repository_id = ?1"),
                        params![repository_id.clone()],
                    )
                    .await?;
                }
                conn.execute(
                    "DELETE FROM repositories WHERE id = ?1",
                    params![repository_id],
                )
                .await?;
            }
            let mut repository_rows = conn
                .query(
                    "INSERT INTO repositories (id, name) VALUES (?1, ?2)
                     RETURNING id, name, created_at",
                    params![Uuid::new_v4().to_string(), name],
                )
                .await?;
            let repository = repository(&one(&mut repository_rows).await?)?;
            let mut branch_rows = conn
                .query(
                    "INSERT INTO branches (id, repository_id, name, base_branch_id, base_position)
                     VALUES (?1, ?2, 'main', NULL, 0)
                     RETURNING id, repository_id, name, base_branch_id, base_position, created_at",
                    params![Uuid::new_v4().to_string(), repository.id.to_string()],
                )
                .await?;
            let main = branch(&one(&mut branch_rows).await?)?;
            Ok::<_, anyhow::Error>((repository, main))
        }
        .await;
        match result {
            Ok(reset) => {
                conn.execute("COMMIT", ()).await?;
                Ok(reset)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
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
        repository(
            &rows
                .next()
                .await?
                .with_context(|| format!("repository {name} does not exist"))?,
        )
    }

    async fn repository_by_id(&self, id: Uuid) -> Result<Repository> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, name, created_at FROM repositories WHERE id = ?1",
                params![id.to_string()],
            )
            .await?;
        repository(
            &rows
                .next()
                .await?
                .with_context(|| format!("repository {id} does not exist"))?,
        )
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

    async fn delete_branch_by_name(&self, repository_id: Uuid, name: &str) -> Result<()> {
        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let id = {
                let mut rows = conn
                    .query(
                        "SELECT id FROM branches WHERE repository_id = ?1 AND name = ?2",
                        params![repository_id.to_string(), name],
                    )
                    .await?;
                match rows.next().await? {
                    Some(row) => text(&row, 0)?,
                    None => return Ok::<(), anyhow::Error>(()),
                }
            };
            let active_conflict = {
                let mut rows = conn
                    .query(
                        "SELECT id, state FROM conflicts
                         WHERE state IN ('open', 'resolving')
                           AND (source_branch_id = ?1 OR target_branch_id = ?1)
                         ORDER BY created_at, id
                         LIMIT 1",
                        params![id.clone()],
                    )
                    .await?;
                match rows.next().await? {
                    Some(row) => Some((text(&row, 0)?, text(&row, 1)?)),
                    None => None,
                }
            };
            if let Some((conflict_id, state)) = active_conflict {
                bail!("cannot delete branch {name}; conflict {conflict_id} is still {state}");
            }
            conn.execute(
                "UPDATE conflicts
             SET source_branch_id = CASE
                     WHEN source_branch_id = ?1 THEN NULL
                     ELSE source_branch_id
                 END,
                 target_branch_id = CASE
                     WHEN target_branch_id = ?1 THEN NULL
                     ELSE target_branch_id
                 END
             WHERE source_branch_id = ?1 OR target_branch_id = ?1",
                params![id.clone()],
            )
            .await?;
            conn.execute(
                "UPDATE conflicts
             SET slice_id = NULL
             WHERE slice_id IN (SELECT id FROM slices WHERE branch_id = ?1)",
                params![id.clone()],
            )
            .await?;
            conn.execute(
                "UPDATE conflicts
             SET source_changeset_id = CASE
                     WHEN source_changeset_id IN (
                         SELECT id FROM changesets WHERE branch_id = ?1
                     ) THEN NULL
                     ELSE source_changeset_id
                 END,
                 resolution_changeset_id = CASE
                     WHEN resolution_changeset_id IN (
                         SELECT id FROM changesets WHERE branch_id = ?1
                     ) THEN NULL
                     ELSE resolution_changeset_id
                 END
             WHERE source_changeset_id IN (
                       SELECT id FROM changesets WHERE branch_id = ?1
                   )
                OR resolution_changeset_id IN (
                       SELECT id FROM changesets WHERE branch_id = ?1
                   )",
                params![id.clone()],
            )
            .await?;
            for table in [
                "graph_cache",
                "source_file_snapshots",
                "slices",
                "operations",
                "changesets",
            ] {
                conn.execute(
                    &format!("DELETE FROM {table} WHERE branch_id = ?1"),
                    params![id.clone()],
                )
                .await?;
            }
            conn.execute(
                "UPDATE branches SET base_branch_id = NULL WHERE base_branch_id = ?1",
                params![id.clone()],
            )
            .await?;
            conn.execute("DELETE FROM branches WHERE id = ?1", params![id])
                .await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                conn.execute("COMMIT", ()).await?;
                Ok(())
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
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
        branch(
            &rows
                .next()
                .await?
                .with_context(|| format!("branch {name} does not exist"))?,
        )
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
        branch(
            &rows
                .next()
                .await?
                .with_context(|| format!("branch {branch_id} does not exist"))?,
        )
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

    async fn append_operations(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        changeset_id: Uuid,
        operations: Vec<PendingOperation>,
    ) -> Result<Vec<OperationRecord>> {
        self.append_operations_transactional(
            repository_id,
            branch_id,
            changeset_id,
            None,
            operations,
        )
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
        self.append_operations_transactional(
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
        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let mut position_rows = conn
                .query(
                    "SELECT COALESCE(MAX(position), 0) FROM operations WHERE branch_id = ?1",
                    params![commit.branch_id.to_string()],
                )
                .await?;
            let current_position = int(&one(&mut position_rows).await?, 0)?;
            if current_position != commit.expected_current_position {
                return Err(BranchAdvanced {
                    branch_id: commit.branch_id,
                    expected: commit.expected_current_position,
                    actual: current_position,
                }
                .into());
            }
            let task_id = Uuid::new_v4();
            conn.execute(
                "INSERT INTO tasks (id, repository_id, title) VALUES (?1, ?2, ?3)",
                params![
                    task_id.to_string(),
                    commit.repository_id.to_string(),
                    commit.task_title
                ],
            )
            .await?;
            let mut changeset_rows = conn
                .query(
                    "INSERT INTO changesets
                        (id, repository_id, task_id, branch_id, title, created_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     RETURNING id, repository_id, task_id, branch_id, title, created_by, created_at",
                    params![
                        Uuid::new_v4().to_string(),
                        commit.repository_id.to_string(),
                        task_id.to_string(),
                        commit.branch_id.to_string(),
                        commit.changeset_title,
                        commit.created_by
                    ],
                )
                .await?;
            let changeset = changeset(&one(&mut changeset_rows).await?)?;
            let mut operations = Vec::with_capacity(commit.operations.len());
            for (position, pending) in (current_position + 1..).zip(commit.operations) {
                let mut rows = conn
                    .query(
                        "INSERT INTO operations
                            (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at",
                        params![
                            Uuid::new_v4().to_string(),
                            commit.repository_id.to_string(),
                            commit.branch_id.to_string(),
                            changeset.id.to_string(),
                            position,
                            pending.op_type,
                            serde_json::to_string(&pending.payload)?
                        ],
                    )
                    .await?;
                operations.push(operation(&one(&mut rows).await?)?);
            }
            Ok::<_, anyhow::Error>(CommittedOperations { operations })
        }
        .await;
        match result {
            Ok(committed) => {
                conn.execute("COMMIT", ()).await?;
                Ok(committed)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
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
        slice(
            &rows
                .next()
                .await?
                .with_context(|| format!("slice {slice_id} does not exist"))?,
        )
    }

    async fn insert_conflict_with_attachment(
        &self,
        conflict: PendingConflict,
        attachment: PendingAttachment,
    ) -> Result<ConflictRecord> {
        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let mut rows = conn
                .query(
                    "INSERT INTO conflicts (
                        id, repository_id, kind, state,
                        source_branch_id, target_branch_id,
                        source_branch_name, target_branch_name,
                        slice_id, source_changeset_id,
                        base_position, target_position,
                        conflicts, proposed_operations, created_by
                     )
                     VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     RETURNING id, repository_id, kind, state,
                               source_branch_id, target_branch_id,
                               source_branch_name, target_branch_name,
                               slice_id, source_changeset_id,
                               base_position, target_position,
                               conflicts, proposed_operations,
                               resolution_changeset_id, acknowledged_drop,
                               created_by, resolved_by, created_at, resolved_at",
                    params![
                        Uuid::new_v4().to_string(),
                        conflict.repository_id.to_string(),
                        conflict.kind.as_str(),
                        conflict.source_branch_id.to_string(),
                        conflict.target_branch_id.to_string(),
                        conflict.source_branch_name,
                        conflict.target_branch_name,
                        conflict.slice_id.map(|id| id.to_string()),
                        conflict.source_changeset_id.map(|id| id.to_string()),
                        conflict.base_position,
                        conflict.target_position,
                        serde_json::to_string(&conflict.conflicts)?,
                        serde_json::to_string(&conflict.proposed_operations)?,
                        conflict.created_by
                    ],
                )
                .await?;
            let record = conflict_record(&one(&mut rows).await?)?;
            conn.execute(
                "INSERT INTO attachments
                    (id, repository_id, entity_type, entity_id, attachment_type, payload)
                 VALUES (?1, ?2, 'conflict', ?3, ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    record.repository_id.to_string(),
                    record.id.to_string(),
                    attachment.attachment_type,
                    serde_json::to_string(&attachment.payload)?
                ],
            )
            .await?;
            Ok::<_, anyhow::Error>(record)
        }
        .await;
        match result {
            Ok(record) => {
                conn.execute("COMMIT", ()).await?;
                Ok(record)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn list_conflicts(&self, repository_id: Uuid) -> Result<Vec<ConflictRecord>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, kind, state,
                        source_branch_id, target_branch_id,
                        source_branch_name, target_branch_name,
                        slice_id, source_changeset_id,
                        base_position, target_position,
                        conflicts, proposed_operations,
                        resolution_changeset_id, acknowledged_drop,
                        created_by, resolved_by, created_at, resolved_at
                 FROM conflicts
                 WHERE repository_id = ?1
                 ORDER BY created_at DESC, id",
                params![repository_id.to_string()],
            )
            .await?;
        collect(&mut rows, conflict_record).await
    }

    async fn conflict_by_id(&self, conflict_id: Uuid) -> Result<ConflictRecord> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, repository_id, kind, state,
                        source_branch_id, target_branch_id,
                        source_branch_name, target_branch_name,
                        slice_id, source_changeset_id,
                        base_position, target_position,
                        conflicts, proposed_operations,
                        resolution_changeset_id, acknowledged_drop,
                        created_by, resolved_by, created_at, resolved_at
                 FROM conflicts
                 WHERE id = ?1",
                params![conflict_id.to_string()],
            )
            .await?;
        conflict_record(
            &rows
                .next()
                .await?
                .with_context(|| format!("conflict {conflict_id} does not exist"))?,
        )
    }

    async fn update_conflict_state(
        &self,
        conflict_id: Uuid,
        state: ConflictState,
    ) -> Result<ConflictRecord> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "UPDATE conflicts
                 SET state = ?2
                 WHERE id = ?1
                 RETURNING id, repository_id, kind, state,
                           source_branch_id, target_branch_id,
                           source_branch_name, target_branch_name,
                           slice_id, source_changeset_id,
                           base_position, target_position,
                           conflicts, proposed_operations,
                           resolution_changeset_id, acknowledged_drop,
                           created_by, resolved_by, created_at, resolved_at",
                params![conflict_id.to_string(), state.as_str()],
            )
            .await?;
        conflict_record(
            &rows
                .next()
                .await?
                .with_context(|| format!("conflict {conflict_id} does not exist"))?,
        )
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
        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let mut position_rows = conn
                .query(
                    "SELECT COALESCE(MAX(position), 0) FROM operations WHERE branch_id = ?1",
                    params![commit.branch_id.to_string()],
                )
                .await?;
            let current_position = int(&one(&mut position_rows).await?, 0)?;
            if current_position != commit.expected_current_position {
                return Err(BranchAdvanced {
                    branch_id: commit.branch_id,
                    expected: commit.expected_current_position,
                    actual: current_position,
                }
                .into());
            }

            let mut conflict_rows = conn
                .query(
                    "SELECT repository_id, target_branch_id, state FROM conflicts WHERE id = ?1",
                    params![conflict_id.to_string()],
                )
                .await?;
            let conflict_row = conflict_rows
                .next()
                .await?
                .with_context(|| format!("conflict {conflict_id} does not exist"))?;
            let conflict_repository_id = uuid(&conflict_row, 0)?;
            let target_branch_id = uuid(&conflict_row, 1)?;
            let state = text(&conflict_row, 2)?;
            if conflict_repository_id != commit.repository_id
                || target_branch_id != commit.branch_id
                || !matches!(state.as_str(), "open" | "resolving")
            {
                bail!("conflict {conflict_id} is not resolvable on the requested branch");
            }

            let task_id = Uuid::new_v4();
            conn.execute(
                "INSERT INTO tasks (id, repository_id, title) VALUES (?1, ?2, ?3)",
                params![
                    task_id.to_string(),
                    commit.repository_id.to_string(),
                    commit.task_title
                ],
            )
            .await?;
            let mut changeset_rows = conn
                .query(
                    "INSERT INTO changesets
                        (id, repository_id, task_id, branch_id, title, created_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     RETURNING id, repository_id, task_id, branch_id, title, created_by, created_at",
                    params![
                        Uuid::new_v4().to_string(),
                        commit.repository_id.to_string(),
                        task_id.to_string(),
                        commit.branch_id.to_string(),
                        commit.changeset_title,
                        commit.created_by
                    ],
                )
                .await?;
            let changeset = changeset(&one(&mut changeset_rows).await?)?;
            let mut operations = Vec::with_capacity(commit.operations.len());
            for (position, pending) in (current_position + 1..).zip(commit.operations) {
                let mut rows = conn
                    .query(
                        "INSERT INTO operations
                            (id, repository_id, branch_id, changeset_id, position, op_type, payload)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         RETURNING id, repository_id, branch_id, changeset_id, position, op_type, payload, created_at",
                        params![
                            Uuid::new_v4().to_string(),
                            commit.repository_id.to_string(),
                            commit.branch_id.to_string(),
                            changeset.id.to_string(),
                            position,
                            pending.op_type,
                            serde_json::to_string(&pending.payload)?
                        ],
                    )
                    .await?;
                operations.push(operation(&one(&mut rows).await?)?);
            }
            conn.execute(
                "INSERT INTO attachments
                    (id, repository_id, entity_type, entity_id, attachment_type, payload)
                 VALUES (?1, ?2, 'changeset', ?3, ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    commit.repository_id.to_string(),
                    changeset.id.to_string(),
                    attachment.attachment_type,
                    serde_json::to_string(&attachment.payload)?
                ],
            )
            .await?;
            let mut conflict_rows = conn
                .query(
                    "UPDATE conflicts
                     SET state = 'resolved', resolution_changeset_id = ?2,
                         acknowledged_drop = ?4, resolved_by = ?3,
                         resolved_at = CURRENT_TIMESTAMP
                     WHERE id = ?1
                     RETURNING id, repository_id, kind, state,
                               source_branch_id, target_branch_id,
                               source_branch_name, target_branch_name,
                               slice_id, source_changeset_id,
                               base_position, target_position,
                               conflicts, proposed_operations,
                               resolution_changeset_id, acknowledged_drop,
                               created_by, resolved_by, created_at, resolved_at",
                    params![
                        conflict_id.to_string(),
                        changeset.id.to_string(),
                        resolved_by,
                        if acknowledged_drop { 1_i64 } else { 0_i64 }
                    ],
                )
                .await?;
            let conflict = conflict_record(&one(&mut conflict_rows).await?)?;
            Ok::<_, anyhow::Error>(CommittedConflictResolution {
                conflict,
                changeset,
                operations,
            })
        }
        .await;
        match result {
            Ok(committed) => {
                conn.execute("COMMIT", ()).await?;
                Ok(committed)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn enqueue_agent_job(
        &self,
        repository_id: Uuid,
        spec: AgentJobSpec,
        request_fingerprint: String,
        now: DateTime<Utc>,
    ) -> Result<AgentJob> {
        let conn = self.conn().await?;
        let tenant_key = spec.tenant_key.clone();
        let idempotency_key = spec.idempotency_key.clone();
        let available_at = sql_timestamp(spec.available_at.unwrap_or(now));
        let now = sql_timestamp(now);
        let mut rows = conn
            .query(
                "INSERT INTO agent_jobs (
                    id, tenant_key, repository_id, kind, idempotency_key,
                    request_fingerprint, payload, state,
                    max_attempts, retry_backoff_seconds, max_backoff_seconds,
                    available_at, created_at, updated_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9, ?10, ?11, ?12, ?12)
                 ON CONFLICT(tenant_key, idempotency_key) DO UPDATE
                 SET idempotency_key = agent_jobs.idempotency_key
                 WHERE agent_jobs.request_fingerprint = excluded.request_fingerprint
                 RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                           attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                           available_at, lease_owner, lease_expires_at, result, error,
                           created_at, updated_at, completed_at",
                params![
                    Uuid::new_v4().to_string(),
                    spec.tenant_key,
                    repository_id.to_string(),
                    spec.kind,
                    spec.idempotency_key,
                    request_fingerprint,
                    serde_json::to_string(&spec.payload)?,
                    spec.max_attempts,
                    spec.retry_backoff_seconds,
                    spec.max_backoff_seconds,
                    available_at,
                    now
                ],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            let mut existing = conn
                .query(
                    "SELECT request_fingerprint FROM agent_jobs
                     WHERE tenant_key = ?1 AND idempotency_key = ?2",
                    params![tenant_key, idempotency_key.clone()],
                )
                .await?;
            if let Some(existing) = existing.next().await?
                && text(&existing, 0)? == "0".repeat(64)
            {
                bail!(
                    "idempotency key {idempotency_key} is already used by a different job request; \
                     the legacy job has no immutable request fingerprint, so use a new idempotency key"
                );
            }
            bail!("idempotency key {idempotency_key} is already used by a different job request");
        };
        agent_job(&row)
    }

    async fn agent_job_by_id(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
    ) -> Result<AgentJob> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                        attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                        available_at, lease_owner, lease_expires_at, result, error,
                        created_at, updated_at, completed_at
                 FROM agent_jobs
                 WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3",
                params![tenant_key, repository_id.to_string(), job_id.to_string()],
            )
            .await?;
        agent_job(&rows.next().await?.with_context(|| {
            format!("agent job {job_id} does not exist in repository {repository_id}")
        })?)
    }

    async fn list_agent_jobs(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
    ) -> Result<Vec<AgentJob>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                        attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                        available_at, lease_owner, lease_expires_at, result, error,
                        created_at, updated_at, completed_at
                 FROM agent_jobs
                 WHERE tenant_key = ?1 AND repository_id = ?2
                 ORDER BY created_at, id",
                params![tenant_key, repository_id.to_string()],
            )
            .await?;
        collect(&mut rows, agent_job).await
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
        let conn = self.conn().await?;
        let now = sql_timestamp(now);
        let lease_expires_at = sql_timestamp(lease_expires_at);
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            conn.execute(
                "UPDATE agent_jobs
                 SET state = 'failed', lease_owner = NULL, lease_token_digest = NULL,
                     lease_expires_at = NULL,
                     error = COALESCE(error, 'lease expired after maximum attempts'),
                     completed_at = ?3, updated_at = ?3
                 WHERE tenant_key = ?1 AND repository_id = ?2
                   AND state = 'leased' AND lease_expires_at <= ?3
                   AND attempt_count >= max_attempts",
                params![tenant_key, repository_id.to_string(), now.clone()],
            )
            .await?;
            let mut candidate_rows = conn
                .query(
                    "SELECT id FROM agent_jobs
                     WHERE tenant_key = ?1 AND repository_id = ?2
                       AND (?3 IS NULL OR kind = ?3)
                       AND attempt_count < max_attempts
                       AND ((state = 'queued' AND available_at <= ?4)
                         OR (state = 'leased' AND lease_expires_at <= ?4))
                     ORDER BY available_at, created_at, id
                     LIMIT 1",
                    params![
                        tenant_key,
                        repository_id.to_string(),
                        kind.map(str::to_string),
                        now.clone()
                    ],
                )
                .await?;
            let Some(candidate) = candidate_rows.next().await? else {
                return Ok::<Option<AgentJob>, anyhow::Error>(None);
            };
            let job_id = text(&candidate, 0)?;
            let mut rows = conn
                .query(
                    "UPDATE agent_jobs
                     SET state = 'leased', attempt_count = attempt_count + 1,
                         lease_owner = ?4, lease_token_digest = ?5, lease_expires_at = ?7,
                         error = NULL, updated_at = ?6
                     WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3
                     RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                               attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                               available_at, lease_owner, lease_expires_at, result, error,
                               created_at, updated_at, completed_at",
                    params![
                        tenant_key,
                        repository_id.to_string(),
                        job_id,
                        owner,
                        lease_token_digest,
                        now.clone(),
                        lease_expires_at
                    ],
                )
                .await?;
            Ok(Some(agent_job(&one(&mut rows).await?)?))
        }
        .await;
        match result {
            Ok(job) => {
                conn.execute("COMMIT", ()).await?;
                Ok(job)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
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
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "UPDATE agent_jobs
                 SET lease_expires_at = ?7, updated_at = ?6
                 WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3
                   AND state = 'leased' AND lease_owner = ?4
                   AND lease_token_digest = ?5 AND lease_expires_at > ?6
                 RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                           attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                           available_at, lease_owner, lease_expires_at, result, error,
                           created_at, updated_at, completed_at",
                params![
                    tenant_key,
                    repository_id.to_string(),
                    job_id.to_string(),
                    owner,
                    lease_token_digest,
                    sql_timestamp(now),
                    sql_timestamp(lease_expires_at)
                ],
            )
            .await?;
        agent_job(
            &rows.next().await?.with_context(|| {
                format!("agent job {job_id} has no active lease owned by {owner}")
            })?,
        )
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
        let conn = self.conn().await?;
        let now = sql_timestamp(now);
        let mut rows = conn
            .query(
                "UPDATE agent_jobs
                 SET state = 'succeeded', result = ?7, error = NULL,
                     lease_owner = NULL, lease_token_digest = NULL, lease_expires_at = NULL,
                     completed_at = ?6, updated_at = ?6
                 WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3
                   AND state = 'leased' AND lease_owner = ?4
                   AND lease_token_digest = ?5 AND lease_expires_at > ?6
                 RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                           attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                           available_at, lease_owner, lease_expires_at, result, error,
                           created_at, updated_at, completed_at",
                params![
                    tenant_key,
                    repository_id.to_string(),
                    job_id.to_string(),
                    owner,
                    lease_token_digest,
                    now,
                    serde_json::to_string(&result)?
                ],
            )
            .await?;
        agent_job(
            &rows.next().await?.with_context(|| {
                format!("agent job {job_id} has no active lease owned by {owner}")
            })?,
        )
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
        let conn = self.conn().await?;
        let now_text = sql_timestamp(now);
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let mut current_rows = conn
                .query(
                    "SELECT id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                            attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                            available_at, lease_owner, lease_expires_at, result, error,
                            created_at, updated_at, completed_at
                     FROM agent_jobs
                     WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3
                       AND state = 'leased' AND lease_owner = ?4
                       AND lease_token_digest = ?5 AND lease_expires_at > ?6",
                    params![
                        tenant_key,
                        repository_id.to_string(),
                        job_id.to_string(),
                        owner,
                        lease_token_digest,
                        now_text.clone()
                    ],
                )
                .await?;
            let current = agent_job(
                &current_rows.next().await?.with_context(|| {
                    format!("agent job {job_id} has no active lease owned by {owner}")
                })?,
            )?;
            let terminal = current.attempt_count >= current.max_attempts;
            let available_at = sql_timestamp(
                now + chrono::Duration::seconds(current.retry_delay_seconds()),
            );
            let mut rows = conn
                .query(
                    "UPDATE agent_jobs
                     SET state = CASE WHEN ?4 = 1 THEN 'failed' ELSE 'queued' END,
                         available_at = CASE WHEN ?4 = 1 THEN available_at ELSE ?5 END,
                         lease_owner = NULL, lease_token_digest = NULL,
                         lease_expires_at = NULL, result = NULL,
                         error = ?6,
                         completed_at = CASE WHEN ?4 = 1 THEN ?7 ELSE NULL END,
                         updated_at = ?7
                     WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3
                     RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                               attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                               available_at, lease_owner, lease_expires_at, result, error,
                               created_at, updated_at, completed_at",
                    params![
                        tenant_key,
                        repository_id.to_string(),
                        job_id.to_string(),
                        if terminal { 1_i64 } else { 0_i64 },
                        available_at,
                        error,
                        now_text
                    ],
                )
                .await?;
            agent_job(&one(&mut rows).await?)
        }
        .await;
        match result {
            Ok(job) => {
                conn.execute("COMMIT", ()).await?;
                Ok(job)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    async fn cancel_agent_job(
        &self,
        tenant_key: &str,
        repository_id: Uuid,
        job_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<AgentJob> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "UPDATE agent_jobs
                 SET state = 'cancelled', lease_owner = NULL, lease_token_digest = NULL,
                     lease_expires_at = NULL,
                     completed_at = COALESCE(completed_at, ?4), updated_at = ?4
                 WHERE tenant_key = ?1 AND repository_id = ?2 AND id = ?3
                   AND state IN ('queued', 'leased', 'cancelled')
                 RETURNING id, tenant_key, repository_id, kind, idempotency_key, payload, state,
                           attempt_count, max_attempts, retry_backoff_seconds, max_backoff_seconds,
                           available_at, lease_owner, lease_expires_at, result, error,
                           created_at, updated_at, completed_at",
                params![
                    tenant_key,
                    repository_id.to_string(),
                    job_id.to_string(),
                    sql_timestamp(now)
                ],
            )
            .await?;
        agent_job(
            &rows
                .next()
                .await?
                .with_context(|| format!("agent job {job_id} cannot be cancelled"))?,
        )
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

    async fn latest_graph_checkpoint(
        &self,
        branch_id: Uuid,
    ) -> Result<Option<StoredGraphCheckpoint>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT operation_count, operation_fingerprint, graph FROM graph_cache
                 WHERE branch_id = ?1
                 ORDER BY operation_count DESC
                 LIMIT 1",
                params![branch_id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(StoredGraphCheckpoint {
                operation_count: int(&row, 0)?,
                operation_fingerprint: text(&row, 1)?,
                graph: json(&row, 2)?,
            })),
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

    async fn list_source_file_snapshots(&self, branch_id: Uuid) -> Result<Vec<SourceFileSnapshot>> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT repository_id, branch_id, path, content_hash, byte_len, handler,
                        file_symbol_id, last_import_position, importer_version, updated_at
                 FROM source_file_snapshots
                 WHERE branch_id = ?1
                 ORDER BY path",
                params![branch_id.to_string()],
            )
            .await?;
        collect(&mut rows, source_file_snapshot).await
    }

    async fn replace_source_file_snapshots(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        snapshots: Vec<PendingSourceFileSnapshot>,
    ) -> Result<()> {
        let conn = self.conn().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            conn.execute(
                "DELETE FROM source_file_snapshots WHERE branch_id = ?1",
                params![branch_id.to_string()],
            )
            .await?;
            for snapshot in snapshots {
                conn.execute(
                    "INSERT INTO source_file_snapshots
                        (repository_id, branch_id, path, content_hash, byte_len, handler,
                         file_symbol_id, last_import_position, importer_version)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        repository_id.to_string(),
                        branch_id.to_string(),
                        snapshot.path,
                        snapshot.content_hash,
                        snapshot.byte_len,
                        snapshot.handler,
                        snapshot.file_symbol_id.map(|id| id.to_string()),
                        snapshot.last_import_position,
                        snapshot.importer_version
                    ],
                )
                .await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                conn.execute("COMMIT", ()).await?;
                Ok(())
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }
}

// ---- row mapping ----

async fn one(rows: &mut turso::Rows) -> Result<Row> {
    let row = rows.next().await?.context("expected a returned row")?;
    if rows.next().await?.is_some() {
        bail!("expected exactly one returned row")
    }
    Ok(row)
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

fn bool_int(row: &Row, i: usize) -> Result<bool> {
    match int(row, i)? {
        0 => Ok(false),
        1 => Ok(true),
        other => bail!("column {i}: expected boolean integer 0 or 1, got {other}"),
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

fn json_opt(row: &Row, i: usize) -> Result<Option<Value>> {
    match text_opt(row, i)? {
        Some(value) => Ok(Some(serde_json::from_str(&value)?)),
        None => Ok(None),
    }
}

fn timestamp(row: &Row, i: usize) -> Result<DateTime<Utc>> {
    let raw = text(row, i)?;
    parse_timestamp(&raw)
}

fn timestamp_opt(row: &Row, i: usize) -> Result<Option<DateTime<Utc>>> {
    match text_opt(row, i)? {
        Some(raw) => Ok(Some(parse_timestamp(&raw)?)),
        None => Ok(None),
    }
}

fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f"))
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f"))
        .with_context(|| format!("unparseable timestamp {raw:?}"))?;
    Ok(naive.and_utc())
}

fn sql_timestamp(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
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

fn source_file_snapshot(row: &Row) -> Result<SourceFileSnapshot> {
    Ok(SourceFileSnapshot {
        repository_id: uuid(row, 0)?,
        branch_id: uuid(row, 1)?,
        path: text(row, 2)?,
        content_hash: text(row, 3)?,
        byte_len: int(row, 4)?,
        handler: text(row, 5)?,
        file_symbol_id: uuid_opt(row, 6)?,
        last_import_position: int(row, 7)?,
        importer_version: text(row, 8)?,
        updated_at: timestamp(row, 9)?,
    })
}

fn conflict_record(row: &Row) -> Result<ConflictRecord> {
    Ok(ConflictRecord {
        id: uuid(row, 0)?,
        repository_id: uuid(row, 1)?,
        kind: text(row, 2)?.parse::<ConflictKind>()?,
        state: text(row, 3)?.parse::<ConflictState>()?,
        source_branch_id: uuid_opt(row, 4)?,
        target_branch_id: uuid_opt(row, 5)?,
        source_branch_name: text(row, 6)?,
        target_branch_name: text(row, 7)?,
        slice_id: uuid_opt(row, 8)?,
        source_changeset_id: uuid_opt(row, 9)?,
        base_position: int(row, 10)?,
        target_position: int(row, 11)?,
        conflicts: serde_json::from_value(json(row, 12)?)?,
        proposed_operations: serde_json::from_value(json(row, 13)?)?,
        resolution_changeset_id: uuid_opt(row, 14)?,
        acknowledged_drop: bool_int(row, 15)?,
        created_by: text(row, 16)?,
        resolved_by: text_opt(row, 17)?,
        created_at: timestamp(row, 18)?,
        resolved_at: timestamp_opt(row, 19)?,
    })
}

fn agent_job(row: &Row) -> Result<AgentJob> {
    Ok(AgentJob {
        id: uuid(row, 0)?,
        tenant_key: text(row, 1)?,
        repository_id: uuid(row, 2)?,
        kind: text(row, 3)?,
        idempotency_key: text(row, 4)?,
        payload: json(row, 5)?,
        state: text(row, 6)?.parse()?,
        attempt_count: int(row, 7)?,
        max_attempts: int(row, 8)?,
        retry_backoff_seconds: int(row, 9)?,
        max_backoff_seconds: int(row, 10)?,
        available_at: timestamp(row, 11)?,
        lease_owner: text_opt(row, 12)?,
        lease_expires_at: timestamp_opt(row, 13)?,
        result: json_opt(row, 14)?,
        error: text_opt(row, 15)?,
        created_at: timestamp(row, 16)?,
        updated_at: timestamp(row, 17)?,
        completed_at: timestamp_opt(row, 18)?,
    })
}
