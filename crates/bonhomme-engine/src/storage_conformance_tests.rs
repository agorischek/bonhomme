//! Backend-neutral storage contract tests.
//!
//! Every scenario below is executed unchanged against embedded Turso and Postgres. The Postgres
//! run is enabled by `BONHOMME_TEST_POSTGRES_URL`; CI always provides it, while contributors can
//! run the full suite locally against any disposable Postgres database.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bonhomme_core::{
    LanguagePlugin, MergeConflict, Operation, RenderedFile, SemanticGraph, Slice, ValidateFuture,
};
use serde_json::json;
use turso::{Builder, Value as TursoValue};
use uuid::Uuid;

use crate::{
    AgentJobSpec, AgentJobState, CacheStatus, ConflictPositions, ConflictState,
    PendingSourceFileSnapshot, Storage,
};

const POSTGRES_TEST_URL: &str = "BONHOMME_TEST_POSTGRES_URL";

struct StubPlugin;

impl LanguagePlugin for StubPlugin {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        graph
            .root_symbols()
            .into_iter()
            .filter(|symbol| symbol.kind == "file")
            .map(|symbol| RenderedFile {
                path: symbol
                    .metadata
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&symbol.name)
                    .to_string(),
                content: symbol.body.clone().unwrap_or_default(),
            })
            .collect()
    }

    fn render_slice(&self, _graph: &SemanticGraph, _base: String, _roots: Vec<Uuid>) -> Slice {
        unimplemented!("not exercised by storage conformance tests")
    }

    fn import(&self, files: &[RenderedFile]) -> anyhow::Result<Vec<Operation>> {
        Ok(files
            .iter()
            .map(|file| Operation::CreateSymbol {
                symbol_id: Uuid::new_v4(),
                parent_id: None,
                kind: "file".to_string(),
                name: file.path.clone(),
                body: Some(file.content.clone()),
                metadata: json!({"path": file.path}),
            })
            .collect())
    }

    fn diff(
        &self,
        _original: &[RenderedFile],
        _modified: &[RenderedFile],
    ) -> anyhow::Result<Vec<Operation>> {
        unimplemented!("not exercised by storage conformance tests")
    }

    fn recover_operations(
        &self,
        _base: &SemanticGraph,
        _scope: &[Uuid],
        _edited: &[RenderedFile],
    ) -> anyhow::Result<Vec<Operation>> {
        Ok(Vec::new())
    }

    fn read_source_tree(&self, _root: &Path) -> anyhow::Result<Vec<RenderedFile>> {
        unimplemented!("not exercised by storage conformance tests")
    }

    fn validate<'a>(&'a self, _files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
enum TestDatabase {
    Turso { path: PathBuf },
    Postgres { url: String },
}

struct Fixture {
    database: TestDatabase,
    storage: Storage,
    run_id: Uuid,
}

impl Fixture {
    async fn turso() -> Self {
        let run_id = Uuid::new_v4();
        let path = std::env::temp_dir().join(format!("bonhomme-conformance-{run_id}.db"));
        Self::connect(TestDatabase::Turso { path }, run_id).await
    }

    async fn postgres(url: String) -> Self {
        Self::connect(TestDatabase::Postgres { url }, Uuid::new_v4()).await
    }

    async fn connect(database: TestDatabase, run_id: Uuid) -> Self {
        let url = match &database {
            TestDatabase::Turso { path } => format!("turso:{}", path.display()),
            TestDatabase::Postgres { url } => url.clone(),
        };
        let storage = Storage::connect(&url, Arc::new(StubPlugin))
            .await
            .unwrap_or_else(|error| panic!("connect conformance database: {error:#}"));
        storage
            .migrate()
            .await
            .unwrap_or_else(|error| panic!("migrate conformance database: {error:#}"));
        Self {
            database,
            storage,
            run_id,
        }
    }

    fn repository_name(&self, scenario: &str) -> String {
        format!("conformance-{}-{scenario}", self.run_id.simple())
    }

    fn trigger_name(&self, purpose: &str) -> String {
        format!("conformance_{purpose}_{}", self.run_id.simple())
    }

    async fn reconnect(&self) -> Storage {
        let url = match &self.database {
            TestDatabase::Turso { path } => format!("turso:{}", path.display()),
            TestDatabase::Postgres { url } => url.clone(),
        };
        let storage = Storage::connect(&url, Arc::new(StubPlugin))
            .await
            .expect("reconnect durable storage");
        storage.migrate().await.expect("migrate after reconnect");
        storage
    }

    async fn install_operation_failure(&self, repository_id: Uuid) {
        let trigger = self.trigger_name("operations");
        match &self.database {
            TestDatabase::Turso { path } => {
                let database = Builder::new_local(path.to_str().expect("UTF-8 Turso test path"))
                    .build()
                    .await
                    .expect("open Turso database for failure injection");
                let connection = database.connect().expect("connect failure injector");
                connection
                    .execute(
                        &format!(
                            "CREATE TRIGGER {trigger}
                             BEFORE INSERT ON operations
                             WHEN NEW.repository_id = '{repository_id}' AND NEW.position = 2
                             BEGIN SELECT RAISE(ABORT, 'injected operation failure'); END"
                        ),
                        (),
                    )
                    .await
                    .expect("install Turso operation failure trigger");
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres failure injector");
                let function = format!("{trigger}_fn");
                sqlx::query(&format!(
                    "CREATE FUNCTION {function}() RETURNS trigger AS $$
                     BEGIN
                       IF NEW.repository_id = '{repository_id}'::uuid AND NEW.position = 2 THEN
                         RAISE EXCEPTION 'injected operation failure';
                       END IF;
                       RETURN NEW;
                     END;
                     $$ LANGUAGE plpgsql"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres operation failure function");
                sqlx::query(&format!(
                    "CREATE TRIGGER {trigger} BEFORE INSERT ON operations
                     FOR EACH ROW EXECUTE FUNCTION {function}()"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres operation failure trigger");
            }
        }
    }

    async fn install_snapshot_failure(&self, repository_id: Uuid) {
        let trigger = self.trigger_name("snapshots");
        match &self.database {
            TestDatabase::Turso { path } => {
                let database = Builder::new_local(path.to_str().expect("UTF-8 Turso test path"))
                    .build()
                    .await
                    .expect("open Turso database for failure injection");
                let connection = database.connect().expect("connect failure injector");
                connection
                    .execute(
                        &format!(
                            "CREATE TRIGGER {trigger}
                             BEFORE INSERT ON source_file_snapshots
                             WHEN NEW.repository_id = '{repository_id}' AND NEW.path = 'fail.ts'
                             BEGIN SELECT RAISE(ABORT, 'injected snapshot failure'); END"
                        ),
                        (),
                    )
                    .await
                    .expect("install Turso snapshot failure trigger");
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres failure injector");
                let function = format!("{trigger}_fn");
                sqlx::query(&format!(
                    "CREATE FUNCTION {function}() RETURNS trigger AS $$
                     BEGIN
                       IF NEW.repository_id = '{repository_id}'::uuid AND NEW.path = 'fail.ts' THEN
                         RAISE EXCEPTION 'injected snapshot failure';
                       END IF;
                       RETURN NEW;
                     END;
                     $$ LANGUAGE plpgsql"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres snapshot failure function");
                sqlx::query(&format!(
                    "CREATE TRIGGER {trigger} BEFORE INSERT ON source_file_snapshots
                     FOR EACH ROW EXECUTE FUNCTION {function}()"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres snapshot failure trigger");
            }
        }
    }

    async fn install_insert_failure(
        &self,
        purpose: &str,
        table: &str,
        turso_condition: &str,
        postgres_condition: &str,
    ) {
        let trigger = self.trigger_name(purpose);
        match &self.database {
            TestDatabase::Turso { path } => {
                let database = Builder::new_local(path.to_str().expect("UTF-8 Turso test path"))
                    .build()
                    .await
                    .expect("open Turso database for failure injection");
                let connection = database.connect().expect("connect failure injector");
                connection
                    .execute(
                        &format!(
                            "CREATE TRIGGER {trigger} BEFORE INSERT ON {table}
                             WHEN {turso_condition}
                             BEGIN SELECT RAISE(ABORT, 'injected transaction failure'); END"
                        ),
                        (),
                    )
                    .await
                    .expect("install Turso transaction failure trigger");
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres failure injector");
                let function = format!("{trigger}_fn");
                sqlx::query(&format!(
                    "CREATE FUNCTION {function}() RETURNS trigger AS $$
                     BEGIN
                       IF {postgres_condition} THEN
                         RAISE EXCEPTION 'injected transaction failure';
                       END IF;
                       RETURN NEW;
                     END;
                     $$ LANGUAGE plpgsql"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres transaction failure function");
                sqlx::query(&format!(
                    "CREATE TRIGGER {trigger} BEFORE INSERT ON {table}
                     FOR EACH ROW EXECUTE FUNCTION {function}()"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres transaction failure trigger");
            }
        }
    }

    async fn install_conflict_resolution_failure(&self, conflict_id: Uuid) {
        let trigger = self.trigger_name("resolution_update");
        match &self.database {
            TestDatabase::Turso { path } => {
                let database = Builder::new_local(path.to_str().expect("UTF-8 Turso test path"))
                    .build()
                    .await
                    .expect("open Turso database for failure injection");
                let connection = database.connect().expect("connect failure injector");
                connection
                    .execute(
                        &format!(
                            "CREATE TRIGGER {trigger} BEFORE UPDATE ON conflicts
                             WHEN NEW.id = '{conflict_id}' AND NEW.state = 'resolved'
                             BEGIN SELECT RAISE(ABORT, 'injected final conflict update failure'); END"
                        ),
                        (),
                    )
                    .await
                    .expect("install Turso conflict resolution failure trigger");
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres failure injector");
                let function = format!("{trigger}_fn");
                sqlx::query(&format!(
                    "CREATE FUNCTION {function}() RETURNS trigger AS $$
                     BEGIN
                       IF NEW.id = '{conflict_id}'::uuid AND NEW.state = 'resolved' THEN
                         RAISE EXCEPTION 'injected final conflict update failure';
                       END IF;
                       RETURN NEW;
                     END;
                     $$ LANGUAGE plpgsql"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres conflict resolution failure function");
                sqlx::query(&format!(
                    "CREATE TRIGGER {trigger} BEFORE UPDATE ON conflicts
                     FOR EACH ROW EXECUTE FUNCTION {function}()"
                ))
                .execute(&pool)
                .await
                .expect("install Postgres conflict resolution failure trigger");
            }
        }
    }

    async fn attachment_count(&self, repository_id: Uuid) -> i64 {
        match &self.database {
            TestDatabase::Turso { path } => {
                let database = Builder::new_local(path.to_str().expect("UTF-8 Turso test path"))
                    .build()
                    .await
                    .expect("open Turso database for attachment inspection");
                let connection = database.connect().expect("connect attachment inspector");
                let mut rows = connection
                    .query(
                        "SELECT COUNT(*) FROM attachments WHERE repository_id = ?1",
                        [repository_id.to_string()],
                    )
                    .await
                    .expect("count Turso attachments");
                let row = rows
                    .next()
                    .await
                    .expect("read Turso attachment count")
                    .expect("Turso attachment count row");
                match row.get_value(0).expect("read Turso count value") {
                    TursoValue::Integer(count) => count,
                    other => panic!("expected integer attachment count, got {other:?}"),
                }
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres attachment inspector");
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM attachments WHERE repository_id = $1",
                )
                .bind(repository_id)
                .fetch_one(&pool)
                .await
                .expect("count Postgres attachments")
            }
        }
    }

    async fn remove_failure_trigger(&self, purpose: &str, table: &str) {
        let trigger = self.trigger_name(purpose);
        match &self.database {
            TestDatabase::Turso { path } => {
                let database = Builder::new_local(path.to_str().expect("UTF-8 Turso test path"))
                    .build()
                    .await
                    .expect("open Turso database for trigger cleanup");
                let connection = database.connect().expect("connect trigger cleanup");
                connection
                    .execute(&format!("DROP TRIGGER IF EXISTS {trigger}"), ())
                    .await
                    .expect("remove Turso failure trigger");
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres trigger cleanup");
                sqlx::query(&format!("DROP TRIGGER IF EXISTS {trigger} ON {table}"))
                    .execute(&pool)
                    .await
                    .expect("remove Postgres failure trigger");
                sqlx::query(&format!("DROP FUNCTION IF EXISTS {trigger}_fn()"))
                    .execute(&pool)
                    .await
                    .expect("remove Postgres failure function");
            }
        }
    }

    async fn cleanup(self) {
        match &self.database {
            TestDatabase::Turso { path } => {
                drop(self.storage);
                let _ = std::fs::remove_file(path);
                let _ = std::fs::remove_file(path.with_extension("db-shm"));
                let _ = std::fs::remove_file(path.with_extension("db-wal"));
            }
            TestDatabase::Postgres { url } => {
                let pool = sqlx::PgPool::connect(url)
                    .await
                    .expect("connect Postgres cleanup");
                sqlx::query("DELETE FROM repositories WHERE name LIKE $1")
                    .bind(format!("conformance-{}-%", self.run_id.simple()))
                    .execute(&pool)
                    .await
                    .expect("clean up Postgres conformance repositories");
            }
        }
    }
}

fn create_symbol(name: impl Into<String>) -> Operation {
    let name = name.into();
    Operation::CreateSymbol {
        symbol_id: Uuid::new_v4(),
        parent_id: None,
        kind: "function".to_string(),
        body: Some(format!("function {name}() {{}}")),
        name,
        metadata: json!({}),
    }
}

fn snapshot(path: &str, content_hash: &str) -> PendingSourceFileSnapshot {
    PendingSourceFileSnapshot {
        path: path.to_string(),
        content_hash: content_hash.to_string(),
        byte_len: 12,
        handler: "typescript".to_string(),
        file_symbol_id: None,
        last_import_position: 1,
        importer_version: "conformance-v1".to_string(),
    }
}

async fn entities_and_branch_history_conform(fixture: &Fixture) {
    let name = fixture.repository_name("entities");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let (same_repository, same_main) = fixture.storage.init_repository(&name).await.unwrap();
    assert_eq!(repository.id, same_repository.id);
    assert_eq!(main.id, same_main.id);

    let task = fixture
        .storage
        .create_task(repository.id, "seed")
        .await
        .unwrap();
    let changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, main.id, "seed", "human")
        .await
        .unwrap();
    let appended = fixture
        .storage
        .append_operations(
            repository.id,
            main.id,
            changeset.id,
            vec![create_symbol("alpha"), create_symbol("beta")],
        )
        .await
        .unwrap();
    assert_eq!(
        appended.iter().map(|op| op.position).collect::<Vec<_>>(),
        [1, 2]
    );
    let tasks = fixture.storage.list_tasks(repository.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task.id);
    let changesets = fixture
        .storage
        .list_changesets(repository.id)
        .await
        .unwrap();
    assert_eq!(changesets.len(), 1);
    assert_eq!(changesets[0].id, changeset.id);

    let feature = fixture
        .storage
        .create_branch(repository.id, "feature", "main")
        .await
        .unwrap();
    assert_eq!(feature.base_branch_id, Some(main.id));
    assert_eq!(feature.base_position, 2);
    fixture
        .storage
        .append_operation(
            repository.id,
            main.id,
            changeset.id,
            create_symbol("postFork"),
        )
        .await
        .unwrap();
    let same_feature = fixture
        .storage
        .create_branch(repository.id, "feature", "main")
        .await
        .unwrap();
    assert_eq!(same_feature.id, feature.id);
    assert_eq!(same_feature.base_position, 2);

    let feature_changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, feature.id, "feature", "agent")
        .await
        .unwrap();
    fixture
        .storage
        .append_operation(
            repository.id,
            feature.id,
            feature_changeset.id,
            create_symbol("featureOnly"),
        )
        .await
        .unwrap();
    let visible = fixture
        .storage
        .collect_branch_operations(feature.id, None)
        .await
        .unwrap();
    assert_eq!(visible.len(), 3, "two inherited operations plus one local");

    let roots = [Uuid::new_v4(), Uuid::new_v4()];
    let slice = fixture
        .storage
        .create_slice(repository.id, feature.id, 1, &roots)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .storage
            .slice_by_id(slice.id)
            .await
            .unwrap()
            .root_symbols,
        roots
    );
    let attachment = fixture
        .storage
        .add_attachment(
            repository.id,
            "branch",
            feature.id,
            "note",
            json!({"labels": ["semantic", "safe"], "score": 2}),
        )
        .await
        .unwrap();
    assert_eq!(attachment.payload["labels"][1], "safe");

    fixture
        .storage
        .replace_source_file_snapshots(
            repository.id,
            feature.id,
            vec![snapshot("src/lib.ts", "sha256:one")],
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .storage
            .list_source_file_snapshots(feature.id)
            .await
            .unwrap()[0]
            .content_hash,
        "sha256:one"
    );

    let miss = fixture
        .storage
        .materialize_branch(&name, "main")
        .await
        .unwrap();
    assert_eq!(miss.cache_status, CacheStatus::Miss);
    let hit = fixture
        .storage
        .materialize_branch(&name, "main")
        .await
        .unwrap();
    assert_eq!(hit.cache_status, CacheStatus::Hit);
    assert_eq!(hit.graph.symbols.len(), miss.graph.symbols.len());

    fixture
        .storage
        .append_operation(
            repository.id,
            main.id,
            changeset.id,
            create_symbol("invalidatesCache"),
        )
        .await
        .unwrap();
    let replacement = fixture
        .storage
        .materialize_branch(&name, "main")
        .await
        .unwrap();
    assert_eq!(replacement.cache_status, CacheStatus::Incremental);
    let replacement_hit = fixture
        .storage
        .materialize_branch(&name, "main")
        .await
        .unwrap();
    assert_eq!(replacement_hit.cache_status, CacheStatus::Hit);
    assert_eq!(replacement_hit.operations.len(), hit.operations.len() + 1);
}

async fn concurrent_same_name_branch_creation_conforms(fixture: &Fixture) {
    let name = fixture.repository_name("concurrent-branch-create");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let task = fixture
        .storage
        .create_task(repository.id, "seed branch")
        .await
        .unwrap();
    let changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, main.id, "seed branch", "test")
        .await
        .unwrap();
    fixture
        .storage
        .append_operation(
            repository.id,
            main.id,
            changeset.id,
            create_symbol("beforeFork"),
        )
        .await
        .unwrap();

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let storage = fixture.storage.clone();
        tasks.spawn(async move {
            storage
                .create_branch(repository.id, "shared-feature", "main")
                .await
        });
    }
    let branches = tokio::time::timeout(Duration::from_secs(10), async {
        let mut branches = Vec::new();
        while let Some(result) = tasks.join_next().await {
            branches.push(result.expect("branch creation task panicked").unwrap());
        }
        branches
    })
    .await
    .expect("concurrent same-name branch creation deadlocked");

    let ids = branches
        .iter()
        .map(|branch| branch.id)
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), 1, "all callers must observe the winning branch");
    assert!(
        branches.iter().all(|branch| branch.base_position == 1),
        "the immutable fork point must match the base history"
    );
    let persisted = fixture.storage.list_branches(repository.id).await.unwrap();
    assert_eq!(
        persisted
            .iter()
            .filter(|branch| branch.name == "shared-feature")
            .count(),
        1
    );
}

async fn atomic_position_allocation_conforms(fixture: &Fixture) {
    let name = fixture.repository_name("positions");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let task = fixture
        .storage
        .create_task(repository.id, "concurrency")
        .await
        .unwrap();
    let changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, main.id, "concurrency", "agents")
        .await
        .unwrap();

    let mut appends = Vec::new();
    for index in 0..12 {
        let storage = fixture.storage.clone();
        appends.push(tokio::spawn(async move {
            storage
                .append_operation(
                    repository.id,
                    main.id,
                    changeset.id,
                    create_symbol(format!("parallel{index}")),
                )
                .await
        }));
    }
    for append in appends {
        append.await.expect("append task panicked").unwrap();
    }

    let operations = fixture
        .storage
        .list_own_operations(main.id, None)
        .await
        .unwrap();
    assert_eq!(
        operations.iter().map(|op| op.position).collect::<Vec<_>>(),
        (1..=12).collect::<Vec<_>>()
    );

    let stale = fixture
        .storage
        .append_operations_if_branch_position(
            repository.id,
            main.id,
            changeset.id,
            11,
            vec![create_symbol("stale")],
        )
        .await;
    assert!(stale.is_err());
    assert_eq!(
        fixture
            .storage
            .list_own_operations(main.id, None)
            .await
            .unwrap()
            .len(),
        12
    );
}

async fn conflict_lifecycle_conforms(fixture: &Fixture) {
    let name = fixture.repository_name("conflicts");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let slice = fixture
        .storage
        .create_slice(repository.id, main.id, 0, &[])
        .await
        .unwrap();
    let conflict = fixture
        .storage
        .create_slice_apply_conflict(
            slice.id,
            &main,
            ConflictPositions::new(0, 0),
            vec![MergeConflict {
                reason: "DUPLICATE_SYMBOL_NAME".to_string(),
                source_operation_id: Uuid::new_v4(),
                target_operation_id: None,
                symbol_id: None,
                detail: "conformance conflict".to_string(),
            }],
            vec![create_symbol("candidate")],
            "conformance",
        )
        .await
        .unwrap();
    assert!(conflict.state.is_active());
    assert!(
        fixture
            .storage
            .delete_branch_by_name(repository.id, "main")
            .await
            .is_err()
    );

    fixture.storage.abandon_conflict(conflict.id).await.unwrap();
    fixture
        .storage
        .delete_branch_by_name(repository.id, "main")
        .await
        .unwrap();
    let retained = fixture.storage.conflict_by_id(conflict.id).await.unwrap();
    assert_eq!(retained.source_branch_id, None);
    assert_eq!(retained.target_branch_id, None);
    assert_eq!(retained.slice_id, None);
    assert_eq!(retained.source_branch_name, "main");
}

async fn operation_batch_rolls_back_after_injected_failure(fixture: &Fixture) {
    let name = fixture.repository_name("operation-rollback");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let task = fixture
        .storage
        .create_task(repository.id, "rollback")
        .await
        .unwrap();
    let changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, main.id, "rollback", "test")
        .await
        .unwrap();
    fixture.install_operation_failure(repository.id).await;

    let result = fixture
        .storage
        .append_operations(
            repository.id,
            main.id,
            changeset.id,
            vec![create_symbol("first"), create_symbol("rejectedSecond")],
        )
        .await;
    fixture
        .remove_failure_trigger("operations", "operations")
        .await;
    assert!(result.is_err(), "the injected second insert must fail");
    assert!(
        fixture
            .storage
            .list_own_operations(main.id, None)
            .await
            .unwrap()
            .is_empty(),
        "the successful first insert must be rolled back"
    );
}

async fn snapshot_replace_rolls_back_after_injected_failure(fixture: &Fixture) {
    let name = fixture.repository_name("snapshot-rollback");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    fixture
        .storage
        .replace_source_file_snapshots(
            repository.id,
            main.id,
            vec![snapshot("original.ts", "sha256:original")],
        )
        .await
        .unwrap();
    fixture.install_snapshot_failure(repository.id).await;

    let result = fixture
        .storage
        .replace_source_file_snapshots(
            repository.id,
            main.id,
            vec![
                snapshot("replacement.ts", "sha256:replacement"),
                snapshot("fail.ts", "sha256:fail"),
            ],
        )
        .await;
    fixture
        .remove_failure_trigger("snapshots", "source_file_snapshots")
        .await;
    assert!(result.is_err(), "the injected second insert must fail");
    let snapshots = fixture
        .storage
        .list_source_file_snapshots(main.id)
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].path, "original.ts");
    assert_eq!(snapshots[0].content_hash, "sha256:original");
}

async fn reset_rolls_back_after_injected_failure(fixture: &Fixture) {
    let name = fixture.repository_name("reset-rollback");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let task = fixture
        .storage
        .create_task(repository.id, "preserved")
        .await
        .unwrap();
    let changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, main.id, "preserved", "test")
        .await
        .unwrap();
    fixture
        .storage
        .append_operation(
            repository.id,
            main.id,
            changeset.id,
            create_symbol("preserved"),
        )
        .await
        .unwrap();
    fixture
        .install_insert_failure(
            "reset",
            "repositories",
            &format!("NEW.name = '{name}'"),
            &format!("NEW.name = '{name}'"),
        )
        .await;

    let result = fixture.storage.reset_repository(&name).await;
    fixture
        .remove_failure_trigger("reset", "repositories")
        .await;
    assert!(result.is_err(), "repository recreation must be rejected");
    let preserved = fixture.storage.repository_by_name(&name).await.unwrap();
    assert_eq!(preserved.id, repository.id, "the delete must roll back");
    assert_eq!(
        fixture
            .storage
            .list_operations(repository.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

async fn conflict_creation_rolls_back_after_injected_failure(fixture: &Fixture) {
    let name = fixture.repository_name("conflict-create-rollback");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let slice = fixture
        .storage
        .create_slice(repository.id, main.id, 0, &[])
        .await
        .unwrap();
    fixture
        .install_insert_failure(
            "conflict_attachment",
            "attachments",
            &format!(
                "NEW.repository_id = '{}' AND NEW.entity_type = 'conflict'",
                repository.id
            ),
            &format!(
                "NEW.repository_id = '{}'::uuid AND NEW.entity_type = 'conflict'",
                repository.id
            ),
        )
        .await;

    let result = fixture
        .storage
        .create_slice_apply_conflict(
            slice.id,
            &main,
            ConflictPositions::new(0, 0),
            vec![],
            vec![create_symbol("proposed")],
            "test",
        )
        .await;
    fixture
        .remove_failure_trigger("conflict_attachment", "attachments")
        .await;
    assert!(
        result.is_err(),
        "attachment insert must reject conflict creation"
    );
    assert!(
        fixture
            .storage
            .list_conflicts(repository.id)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn merge_commit_rolls_back_after_injected_failure(fixture: &Fixture) {
    let name = fixture.repository_name("merge-rollback");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let seed_task = fixture
        .storage
        .create_task(repository.id, "seed")
        .await
        .unwrap();
    let seed_changeset = fixture
        .storage
        .create_changeset(repository.id, seed_task.id, main.id, "seed", "test")
        .await
        .unwrap();
    fixture
        .storage
        .append_operation(
            repository.id,
            main.id,
            seed_changeset.id,
            create_symbol("base"),
        )
        .await
        .unwrap();
    let feature = fixture
        .storage
        .create_branch(repository.id, "feature", "main")
        .await
        .unwrap();
    let feature_task = fixture
        .storage
        .create_task(repository.id, "feature")
        .await
        .unwrap();
    let feature_changeset = fixture
        .storage
        .create_changeset(
            repository.id,
            feature_task.id,
            feature.id,
            "feature",
            "test",
        )
        .await
        .unwrap();
    fixture
        .storage
        .append_operation(
            repository.id,
            feature.id,
            feature_changeset.id,
            create_symbol("featureOnly"),
        )
        .await
        .unwrap();
    let tasks_before = fixture
        .storage
        .list_tasks(repository.id)
        .await
        .unwrap()
        .len();
    let changesets_before = fixture
        .storage
        .list_changesets(repository.id)
        .await
        .unwrap()
        .len();
    fixture
        .install_insert_failure(
            "merge_operation",
            "operations",
            &format!("NEW.branch_id = '{}' AND NEW.position = 2", main.id),
            &format!("NEW.branch_id = '{}'::uuid AND NEW.position = 2", main.id),
        )
        .await;

    let result = fixture.storage.merge_branch(&name, "feature", "main").await;
    fixture
        .remove_failure_trigger("merge_operation", "operations")
        .await;
    assert!(result.is_err(), "merged operation insert must fail");
    assert_eq!(
        fixture
            .storage
            .list_tasks(repository.id)
            .await
            .unwrap()
            .len(),
        tasks_before
    );
    assert_eq!(
        fixture
            .storage
            .list_changesets(repository.id)
            .await
            .unwrap()
            .len(),
        changesets_before
    );
    assert_eq!(
        fixture
            .storage
            .list_own_operations(main.id, None)
            .await
            .unwrap()
            .len(),
        1
    );
}

async fn conflict_resolution_rolls_back_after_injected_failure(fixture: &Fixture) {
    let name = fixture.repository_name("resolution-rollback");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let slice = fixture
        .storage
        .create_slice(repository.id, main.id, 0, &[])
        .await
        .unwrap();
    let conflict = fixture
        .storage
        .create_slice_apply_conflict(
            slice.id,
            &main,
            ConflictPositions::new(0, 0),
            vec![],
            vec![],
            "test",
        )
        .await
        .unwrap();
    let tasks_before = fixture
        .storage
        .list_tasks(repository.id)
        .await
        .unwrap()
        .len();
    let changesets_before = fixture
        .storage
        .list_changesets(repository.id)
        .await
        .unwrap()
        .len();
    let attachments_before = fixture.attachment_count(repository.id).await;
    fixture
        .install_conflict_resolution_failure(conflict.id)
        .await;

    let result = fixture
        .storage
        .resolve_conflict(
            conflict.id,
            vec![RenderedFile {
                path: "resolved.txt".to_string(),
                content: "resolved content".to_string(),
            }],
            "resolver",
            true,
        )
        .await;
    fixture
        .remove_failure_trigger("resolution_update", "conflicts")
        .await;
    assert!(result.is_err(), "final conflict update must fail");
    assert_eq!(
        fixture
            .storage
            .conflict_by_id(conflict.id)
            .await
            .unwrap()
            .state,
        ConflictState::Open
    );
    assert_eq!(
        fixture
            .storage
            .list_tasks(repository.id)
            .await
            .unwrap()
            .len(),
        tasks_before
    );
    assert_eq!(
        fixture
            .storage
            .list_changesets(repository.id)
            .await
            .unwrap()
            .len(),
        changesets_before
    );
    assert!(
        fixture
            .storage
            .list_own_operations(main.id, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.attachment_count(repository.id).await,
        attachments_before,
        "the resolution attachment inserted before the final update must roll back"
    );
}

async fn reset_cascade_conforms(fixture: &Fixture) {
    let name = fixture.repository_name("reset");
    let (repository, main) = fixture.storage.init_repository(&name).await.unwrap();
    let task = fixture
        .storage
        .create_task(repository.id, "reset")
        .await
        .unwrap();
    let changeset = fixture
        .storage
        .create_changeset(repository.id, task.id, main.id, "reset", "test")
        .await
        .unwrap();
    fixture
        .storage
        .append_operation(
            repository.id,
            main.id,
            changeset.id,
            create_symbol("beforeReset"),
        )
        .await
        .unwrap();
    fixture
        .storage
        .replace_source_file_snapshots(
            repository.id,
            main.id,
            vec![snapshot("before.ts", "sha256:before")],
        )
        .await
        .unwrap();

    let (fresh_repository, fresh_main) = fixture.storage.reset_repository(&name).await.unwrap();
    assert_ne!(fresh_repository.id, repository.id);
    assert!(
        fixture
            .storage
            .list_operations(fresh_repository.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .storage
            .list_branches(fresh_repository.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        fixture
            .storage
            .list_source_file_snapshots(fresh_main.id)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn agent_job_lifecycle_conforms(fixture: &Fixture) {
    let name = fixture.repository_name("job-lifecycle");
    let (repository, _) = fixture.storage.init_repository(&name).await.unwrap();
    let tenant = format!("tenant-{}-lifecycle", fixture.run_id.simple());
    let other_tenant = format!("tenant-{}-other", fixture.run_id.simple());
    let spec = AgentJobSpec::new(
        &tenant,
        "semantic_merge",
        "request-1",
        json!({"branch": "feature", "priority": 4}),
    );
    let queued = fixture
        .storage
        .enqueue_agent_job(repository.id, spec.clone())
        .await
        .unwrap();
    assert_eq!(queued.state, AgentJobState::Queued);
    assert_eq!(queued.attempt_count, 0);
    assert_eq!(queued.tenant_key, tenant);

    let duplicate = fixture
        .storage
        .enqueue_agent_job(repository.id, spec.clone())
        .await
        .unwrap();
    assert_eq!(
        duplicate.id, queued.id,
        "enqueue must be idempotent per tenant"
    );
    let (other_repository, _) = fixture
        .storage
        .init_repository(&fixture.repository_name("job-lifecycle-other-repo"))
        .await
        .unwrap();
    assert!(
        fixture
            .storage
            .enqueue_agent_job(other_repository.id, spec.clone())
            .await
            .is_err(),
        "tenant idempotency keys cannot silently alias a job in another repository"
    );
    let mut conflicting = spec.clone();
    conflicting.payload = json!({"branch": "different"});
    assert!(
        fixture
            .storage
            .enqueue_agent_job(repository.id, conflicting)
            .await
            .is_err(),
        "an idempotency key cannot alias a different request"
    );
    let mut rescheduled = spec;
    rescheduled.available_at = Some(chrono::Utc::now() + chrono::Duration::hours(1));
    assert!(
        fixture
            .storage
            .enqueue_agent_job(repository.id, rescheduled)
            .await
            .is_err(),
        "an idempotency key cannot silently alias a different explicit schedule"
    );
    let scheduled_at = chrono::Utc::now() + chrono::Duration::hours(2);
    let mut scheduled_spec = AgentJobSpec::new(
        &tenant,
        "semantic_merge",
        "explicit-schedule",
        json!({"branch": "scheduled"}),
    );
    scheduled_spec.available_at = Some(scheduled_at);
    let scheduled = fixture
        .storage
        .enqueue_agent_job(repository.id, scheduled_spec.clone())
        .await
        .unwrap();
    let identical_schedule = fixture
        .storage
        .enqueue_agent_job(repository.id, scheduled_spec.clone())
        .await
        .unwrap();
    assert_eq!(scheduled.id, identical_schedule.id);
    let mut omitted_schedule = scheduled_spec.clone();
    omitted_schedule.available_at = None;
    assert!(
        fixture
            .storage
            .enqueue_agent_job(repository.id, omitted_schedule)
            .await
            .is_err(),
        "an omitted schedule cannot alias an explicitly scheduled request"
    );
    scheduled_spec.available_at = Some(scheduled_at + chrono::Duration::minutes(1));
    assert!(
        fixture
            .storage
            .enqueue_agent_job(repository.id, scheduled_spec)
            .await
            .is_err(),
        "a different explicit schedule cannot alias the original request"
    );
    assert!(
        fixture
            .storage
            .enqueue_agent_job(
                repository.id,
                AgentJobSpec::new("x".repeat(129), "kind", "bounded", json!({})),
            )
            .await
            .is_err(),
        "tenant keys are bounded before reaching an indexed database column"
    );

    let other = fixture
        .storage
        .enqueue_agent_job(
            repository.id,
            AgentJobSpec::new(
                &other_tenant,
                "semantic_merge",
                "request-1",
                json!({"branch": "other-tenant"}),
            ),
        )
        .await
        .unwrap();
    assert_ne!(other.id, queued.id, "idempotency keys are tenant-scoped");
    assert_eq!(
        fixture
            .storage
            .list_agent_jobs(&tenant, repository.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        fixture
            .storage
            .agent_job_by_id(&other_tenant, repository.id, queued.id)
            .await
            .is_err(),
        "a tenant cannot read another tenant's job"
    );
    assert!(
        fixture
            .storage
            .lease_agent_job(
                &tenant,
                repository.id,
                "worker-a",
                Some("unhandled_kind"),
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .storage
            .lease_agent_job(
                &tenant,
                repository.id,
                &"w".repeat(129),
                Some("semantic_merge"),
                Duration::from_secs(30),
            )
            .await
            .is_err(),
        "lease owner identifiers are bounded"
    );

    let leased = fixture
        .storage
        .lease_agent_job(
            &tenant,
            repository.id,
            "worker-a",
            Some("semantic_merge"),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(leased.job.id, queued.id);
    assert_eq!(leased.job.state, AgentJobState::Leased);
    assert_eq!(leased.job.attempt_count, 1);
    assert!(
        !serde_json::to_string(&leased.job)
            .unwrap()
            .contains(&leased.lease_token),
        "ordinary job records must not disclose lease credentials"
    );
    assert!(
        fixture
            .storage
            .heartbeat_agent_job(
                &tenant,
                repository.id,
                leased.job.id,
                "wrong-worker",
                &leased.lease_token,
                Duration::from_secs(30),
            )
            .await
            .is_err()
    );
    assert!(
        fixture
            .storage
            .heartbeat_agent_job(
                &tenant,
                repository.id,
                leased.job.id,
                "worker-a",
                "invalid-possession-token",
                Duration::from_secs(30),
            )
            .await
            .is_err(),
        "knowing the owner identifier is insufficient without the lease token"
    );
    let heartbeat = fixture
        .storage
        .heartbeat_agent_job(
            &tenant,
            repository.id,
            leased.job.id,
            "worker-a",
            &leased.lease_token,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(heartbeat.lease_expires_at > leased.job.lease_expires_at);
    let succeeded = fixture
        .storage
        .succeed_agent_job(
            &tenant,
            repository.id,
            leased.job.id,
            "worker-a",
            &leased.lease_token,
            json!({"commit": "abc123"}),
        )
        .await
        .unwrap();
    assert_eq!(succeeded.state, AgentJobState::Succeeded);
    assert_eq!(succeeded.result, Some(json!({"commit": "abc123"})));
    assert!(succeeded.completed_at.is_some());
    let terminal_duplicate = fixture
        .storage
        .enqueue_agent_job(
            repository.id,
            AgentJobSpec::new(
                &tenant,
                "semantic_merge",
                "request-1",
                json!({"branch": "feature", "priority": 4}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(terminal_duplicate.id, succeeded.id);
    assert!(
        fixture
            .storage
            .cancel_agent_job(&tenant, repository.id, succeeded.id)
            .await
            .is_err(),
        "successful jobs are immutable terminal records"
    );

    let cancelled = fixture
        .storage
        .cancel_agent_job(&other_tenant, repository.id, other.id)
        .await
        .unwrap();
    assert_eq!(cancelled.state, AgentJobState::Cancelled);
    assert_eq!(
        fixture
            .storage
            .cancel_agent_job(&other_tenant, repository.id, other.id)
            .await
            .unwrap()
            .state,
        AgentJobState::Cancelled,
        "cancellation is idempotent"
    );
}

async fn agent_job_retry_and_expiry_conform(fixture: &Fixture) {
    let name = fixture.repository_name("job-retry");
    let (repository, _) = fixture.storage.init_repository(&name).await.unwrap();
    let tenant = format!("tenant-{}-retry", fixture.run_id.simple());

    let mut backoff_spec = AgentJobSpec::new(
        &tenant,
        "benchmark",
        "backoff",
        json!({"scenario": "rename"}),
    );
    backoff_spec.max_attempts = 5;
    backoff_spec.retry_backoff_seconds = 2;
    backoff_spec.max_backoff_seconds = 3;
    let backoff_job = fixture
        .storage
        .enqueue_agent_job(repository.id, backoff_spec.clone())
        .await
        .unwrap();
    let leased = fixture
        .storage
        .lease_agent_job(
            &tenant,
            repository.id,
            "retry-worker",
            Some("benchmark"),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    let requeued = fixture
        .storage
        .fail_agent_job(
            &tenant,
            repository.id,
            leased.job.id,
            "retry-worker",
            &leased.lease_token,
            "compiler unavailable",
        )
        .await
        .unwrap();
    assert_eq!(requeued.state, AgentJobState::Queued);
    assert_eq!(requeued.error.as_deref(), Some("compiler unavailable"));
    let requeued_duplicate = fixture
        .storage
        .enqueue_agent_job(repository.id, backoff_spec)
        .await
        .unwrap();
    assert_eq!(requeued_duplicate.id, requeued.id);
    assert_eq!(
        (requeued.available_at - requeued.updated_at).num_seconds(),
        2
    );

    let mut oversized_spec = AgentJobSpec::new(
        &tenant,
        "oversized_failure",
        "oversized-failure",
        json!({"attempts": 1}),
    );
    oversized_spec.max_attempts = 1;
    fixture
        .storage
        .enqueue_agent_job(repository.id, oversized_spec)
        .await
        .unwrap();
    let oversized_lease = fixture
        .storage
        .lease_agent_job(
            &tenant,
            repository.id,
            "oversized-worker",
            Some("oversized_failure"),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    let oversized_failed = fixture
        .storage
        .fail_agent_job(
            &tenant,
            repository.id,
            oversized_lease.job.id,
            "oversized-worker",
            &oversized_lease.lease_token,
            &"é".repeat(16 * 1024),
        )
        .await
        .unwrap();
    assert_eq!(oversized_failed.state, AgentJobState::Failed);
    let stored_error = oversized_failed.error.unwrap();
    assert!(stored_error.len() <= 16 * 1024);
    assert!(stored_error.ends_with("[diagnostic truncated]"));

    let mut later_attempt = backoff_job;
    later_attempt.attempt_count = 20;
    assert_eq!(
        later_attempt.retry_delay_seconds(),
        3,
        "retry delay is capped"
    );
    assert!(
        fixture
            .storage
            .lease_agent_job(
                &tenant,
                repository.id,
                "too-early",
                Some("benchmark"),
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .is_none(),
        "backoff must make the job temporarily unavailable"
    );

    let mut immediate_spec = AgentJobSpec::new(
        &tenant,
        "immediate_retry",
        "immediate-retry",
        json!({"attempts": 2}),
    );
    immediate_spec.max_attempts = 2;
    immediate_spec.retry_backoff_seconds = 0;
    immediate_spec.max_backoff_seconds = 0;
    fixture
        .storage
        .enqueue_agent_job(repository.id, immediate_spec)
        .await
        .unwrap();
    for expected_attempt in 1..=2 {
        let leased = fixture
            .storage
            .lease_agent_job(
                &tenant,
                repository.id,
                "failure-worker",
                Some("immediate_retry"),
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leased.job.attempt_count, expected_attempt);
        let failed = fixture
            .storage
            .fail_agent_job(
                &tenant,
                repository.id,
                leased.job.id,
                "failure-worker",
                &leased.lease_token,
                &format!("attempt {expected_attempt} failed"),
            )
            .await
            .unwrap();
        let expected_state = if expected_attempt == 2 {
            AgentJobState::Failed
        } else {
            AgentJobState::Queued
        };
        assert_eq!(failed.state, expected_state);
        if failed.state == AgentJobState::Failed {
            assert_eq!(failed.error.as_deref(), Some("attempt 2 failed"));
            assert!(failed.completed_at.is_some());
        }
    }

    let mut expiry_spec =
        AgentJobSpec::new(&tenant, "expired_lease", "expiry", json!({"work": true}));
    expiry_spec.max_attempts = 2;
    expiry_spec.retry_backoff_seconds = 0;
    expiry_spec.max_backoff_seconds = 0;
    let expiry_job = fixture
        .storage
        .enqueue_agent_job(repository.id, expiry_spec)
        .await
        .unwrap();
    let first = fixture
        .storage
        .lease_agent_job(
            &tenant,
            repository.id,
            "crashed-worker",
            Some("expired_lease"),
            Duration::from_millis(2),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.job.id, expiry_job.id);
    tokio::time::sleep(Duration::from_millis(8)).await;
    let reclaimed = fixture
        .storage
        .lease_agent_job(
            &tenant,
            repository.id,
            "recovery-worker",
            Some("expired_lease"),
            Duration::from_millis(2),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.job.id, expiry_job.id);
    assert_eq!(reclaimed.job.attempt_count, 2);
    assert!(
        fixture
            .storage
            .heartbeat_agent_job(
                &tenant,
                repository.id,
                reclaimed.job.id,
                "crashed-worker",
                &first.lease_token,
                Duration::from_secs(1),
            )
            .await
            .is_err(),
        "the old lease owner must lose authority after reclamation"
    );
    assert!(
        fixture
            .storage
            .heartbeat_agent_job(
                &tenant,
                repository.id,
                reclaimed.job.id,
                "recovery-worker",
                &first.lease_token,
                Duration::from_secs(1),
            )
            .await
            .is_err(),
        "reclamation must rotate the possession token"
    );
    tokio::time::sleep(Duration::from_millis(8)).await;
    assert!(
        fixture
            .storage
            .lease_agent_job(
                &tenant,
                repository.id,
                "third-worker",
                Some("expired_lease"),
                Duration::from_secs(1),
            )
            .await
            .unwrap()
            .is_none()
    );
    let exhausted = fixture
        .storage
        .agent_job_by_id(&tenant, repository.id, expiry_job.id)
        .await
        .unwrap();
    assert_eq!(exhausted.state, AgentJobState::Failed);
    assert!(exhausted.state.is_terminal());
    assert!(exhausted.completed_at.is_some());
    assert!(exhausted.error.unwrap().contains("maximum attempts"));
}

async fn agent_job_atomic_leasing_and_restart_conform(fixture: &Fixture) {
    let name = fixture.repository_name("job-atomic");
    let (repository, _) = fixture.storage.init_repository(&name).await.unwrap();
    let tenant = format!("tenant-{}-atomic", fixture.run_id.simple());
    for index in 0..8 {
        fixture
            .storage
            .enqueue_agent_job(
                repository.id,
                AgentJobSpec::new(
                    &tenant,
                    "parallel",
                    format!("parallel-{index}"),
                    json!({"index": index}),
                ),
            )
            .await
            .unwrap();
    }

    let mut leases = Vec::new();
    for index in 0..8 {
        let storage = fixture.storage.clone();
        let tenant = tenant.clone();
        leases.push(tokio::spawn(async move {
            storage
                .lease_agent_job(
                    &tenant,
                    repository.id,
                    &format!("parallel-worker-{index}"),
                    Some("parallel"),
                    Duration::from_secs(30),
                )
                .await
        }));
    }
    let mut leased_ids = HashSet::new();
    for lease in leases {
        let job = lease
            .await
            .expect("lease task panicked")
            .unwrap()
            .expect("each worker should receive one job");
        assert!(leased_ids.insert(job.job.id), "a job was leased twice");
    }
    assert_eq!(leased_ids.len(), 8);
    assert!(
        fixture
            .storage
            .lease_agent_job(
                &tenant,
                repository.id,
                "ninth-worker",
                Some("parallel"),
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .is_none()
    );

    let restart_tenant = format!("tenant-{}-restart", fixture.run_id.simple());
    let durable = fixture
        .storage
        .enqueue_agent_job(
            repository.id,
            AgentJobSpec::new(
                &restart_tenant,
                "restart",
                "durable-request",
                json!({"persists": true}),
            ),
        )
        .await
        .unwrap();
    let reconnected = fixture.reconnect().await;
    let after_restart = reconnected
        .agent_job_by_id(&restart_tenant, repository.id, durable.id)
        .await
        .unwrap();
    assert_eq!(after_restart.payload, json!({"persists": true}));
    let leased = reconnected
        .lease_agent_job(
            &restart_tenant,
            repository.id,
            "restart-worker",
            Some("restart"),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    reconnected
        .cancel_agent_job(&restart_tenant, repository.id, leased.job.id)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .storage
            .agent_job_by_id(&restart_tenant, repository.id, durable.id)
            .await
            .unwrap()
            .state,
        AgentJobState::Cancelled,
        "terminal state written after reconnect must be visible to the original client"
    );
}

async fn run_storage_conformance(fixture: Fixture) {
    entities_and_branch_history_conform(&fixture).await;
    concurrent_same_name_branch_creation_conforms(&fixture).await;
    atomic_position_allocation_conforms(&fixture).await;
    conflict_lifecycle_conforms(&fixture).await;
    operation_batch_rolls_back_after_injected_failure(&fixture).await;
    snapshot_replace_rolls_back_after_injected_failure(&fixture).await;
    reset_rolls_back_after_injected_failure(&fixture).await;
    conflict_creation_rolls_back_after_injected_failure(&fixture).await;
    merge_commit_rolls_back_after_injected_failure(&fixture).await;
    conflict_resolution_rolls_back_after_injected_failure(&fixture).await;
    agent_job_lifecycle_conforms(&fixture).await;
    agent_job_retry_and_expiry_conform(&fixture).await;
    agent_job_atomic_leasing_and_restart_conform(&fixture).await;
    reset_cascade_conforms(&fixture).await;
    fixture.cleanup().await;
}

#[tokio::test]
async fn turso_storage_conformance() {
    run_storage_conformance(Fixture::turso().await).await;
}

#[tokio::test]
async fn postgres_storage_conformance() {
    let Ok(url) = std::env::var(POSTGRES_TEST_URL) else {
        eprintln!(
            "skipping Postgres conformance; set {POSTGRES_TEST_URL} to a disposable database URL"
        );
        return;
    };
    run_storage_conformance(Fixture::postgres(url).await).await;
}
