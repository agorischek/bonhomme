//! End-to-end coverage of the embedded Turso backend, exercised through the public `Storage` API
//! against an in-memory database (no server required). This proves the SQLite-dialect SQL, the
//! TEXT/JSON round-tripping, the single-statement position allocation, the immutable fork point,
//! the graph cache, and the explicit-cascade reset all behave like the Postgres backend.

use std::path::Path;
use std::sync::Arc;

use bonhomme_core::{
    LanguagePlugin, MergeConflict, Operation, RenderedFile, SemanticGraph, Slice, ValidateFuture,
};
use serde_json::json;
use turso::{Builder, params};
use uuid::Uuid;

use crate::{AgentJobSpec, CacheStatus, CheckpointRejection, PendingSourceFileSnapshot, Storage};

/// A do-nothing language plugin: materialization only needs `render`, and these tests don't touch
/// import/diff/validate.
struct StubPlugin;

impl LanguagePlugin for StubPlugin {
    fn render(&self, _graph: &SemanticGraph) -> Vec<RenderedFile> {
        Vec::new()
    }
    fn render_slice(&self, _g: &SemanticGraph, _base: String, _roots: Vec<Uuid>) -> Slice {
        unimplemented!("not exercised by storage tests")
    }
    fn import(&self, _files: &[RenderedFile]) -> anyhow::Result<Vec<Operation>> {
        unimplemented!("not exercised by storage tests")
    }
    fn diff(&self, _o: &[RenderedFile], _m: &[RenderedFile]) -> anyhow::Result<Vec<Operation>> {
        unimplemented!("not exercised by storage tests")
    }
    fn recover_operations(
        &self,
        _base: &SemanticGraph,
        _scope: &[Uuid],
        _edited: &[RenderedFile],
    ) -> anyhow::Result<Vec<Operation>> {
        unimplemented!("not exercised by storage tests")
    }
    fn read_source_tree(&self, _root: &Path) -> anyhow::Result<Vec<RenderedFile>> {
        unimplemented!("not exercised by storage tests")
    }
    fn validate<'a>(&'a self, _files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        unimplemented!("not exercised by storage tests")
    }
}

fn create_symbol(name: &str) -> Operation {
    Operation::CreateSymbol {
        symbol_id: Uuid::new_v4(),
        parent_id: None,
        kind: "function".into(),
        name: name.into(),
        body: Some(format!("function {name}() {{}}")),
        metadata: json!({}),
    }
}

async fn memory_storage() -> Storage {
    let storage = Storage::connect(":memory:", Arc::new(StubPlugin))
        .await
        .expect("connect to in-memory turso");
    storage.migrate().await.expect("run turso migrations");
    storage
}

#[tokio::test]
async fn migrations_upgrade_pre_token_agent_jobs_table() {
    let path = std::env::temp_dir().join(format!("bonhomme-pre-token-{}.db", Uuid::new_v4()));
    let legacy_job_id = Uuid::new_v4();
    let legacy_repository_id = Uuid::new_v4();
    let database = Builder::new_local(path.to_str().expect("UTF-8 temp path"))
        .build()
        .await
        .expect("create pre-token database");
    let connection = database.connect().expect("connect pre-token database");
    connection
        .execute(
            "CREATE TABLE agent_jobs (
                id TEXT PRIMARY KEY,
                tenant_key TEXT NOT NULL,
                repository_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                payload TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued',
                attempt_count INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL,
                retry_backoff_seconds INTEGER NOT NULL,
                max_backoff_seconds INTEGER NOT NULL,
                available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                lease_owner TEXT,
                lease_expires_at TEXT,
                result TEXT,
                error TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                completed_at TEXT,
                UNIQUE (tenant_key, idempotency_key)
            )",
            (),
        )
        .await
        .expect("create old agent_jobs shape");
    connection
        .execute(
            "INSERT INTO agent_jobs (
                id, tenant_key, repository_id, kind, idempotency_key, payload,
                max_attempts, retry_backoff_seconds, max_backoff_seconds
             ) VALUES (?1, 'legacy-tenant', ?2, 'legacy-kind', 'legacy-key', '{}', 3, 1, 60)",
            params![legacy_job_id.to_string(), legacy_repository_id.to_string()],
        )
        .await
        .expect("insert pre-fingerprint legacy job");
    drop(connection);
    drop(database);

    let storage = Storage::connect(&format!("turso:{}", path.display()), Arc::new(StubPlugin))
        .await
        .expect("connect old local database");
    storage.migrate().await.expect("upgrade old local database");
    let legacy_retry = storage
        .enqueue_agent_job(
            legacy_repository_id,
            AgentJobSpec::new("legacy-tenant", "legacy-kind", "legacy-key", json!({})),
        )
        .await
        .expect_err("legacy idempotency rows must not silently alias an unverifiable request");
    assert!(
        format!("{legacy_retry:#}").contains("legacy job has no immutable request fingerprint"),
        "legacy retry rejection should tell callers to choose a new key: {legacy_retry:#}"
    );
    let (repository, _) = storage.init_repository("upgrade").await.unwrap();
    let job = storage
        .enqueue_agent_job(
            repository.id,
            AgentJobSpec::new("tenant", "kind", "key", json!({})),
        )
        .await
        .expect("enqueue after upgrade");
    let leased = storage
        .lease_agent_job(
            "tenant",
            repository.id,
            "worker",
            None,
            std::time::Duration::from_secs(30),
        )
        .await
        .expect("lease after upgrade")
        .expect("queued job");
    assert_eq!(leased.job.id, job.id);
    assert_eq!(leased.lease_token.len(), 64);

    let inspection_database = Builder::new_local(path.to_str().expect("UTF-8 temp path"))
        .build()
        .await
        .expect("open upgraded database for constraint checks");
    let inspection = inspection_database
        .connect()
        .expect("connect upgraded database");
    assert!(
        inspection
            .execute(
                "UPDATE agent_jobs SET state = 'unknown' WHERE id = ?1",
                params![job.id.to_string()],
            )
            .await
            .is_err(),
        "legacy tables must gain the agent job state constraint"
    );
    assert!(
        inspection
            .execute(
                "UPDATE agent_jobs SET lease_token_digest = ?2 WHERE id = ?1",
                params![job.id.to_string(), "é".repeat(64)],
            )
            .await
            .is_err(),
        "lease token digests are constrained by bytes, not characters"
    );
}

#[tokio::test]
async fn concurrent_migrations_serialize_legacy_agent_job_upgrade() {
    let path =
        std::env::temp_dir().join(format!("bonhomme-concurrent-upgrade-{}.db", Uuid::new_v4()));
    let database = Builder::new_local(path.to_str().expect("UTF-8 temp path"))
        .build()
        .await
        .expect("create legacy database");
    database
        .connect()
        .expect("connect legacy database")
        .execute(
            "CREATE TABLE agent_jobs (
                id TEXT PRIMARY KEY, tenant_key TEXT NOT NULL, repository_id TEXT NOT NULL,
                kind TEXT NOT NULL, idempotency_key TEXT NOT NULL, payload TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued', attempt_count INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL, retry_backoff_seconds INTEGER NOT NULL,
                max_backoff_seconds INTEGER NOT NULL, available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                lease_owner TEXT, lease_expires_at TEXT, result TEXT, error TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, completed_at TEXT,
                UNIQUE (tenant_key, idempotency_key)
            )",
            (),
        )
        .await
        .expect("create legacy agent_jobs table");
    drop(database);

    let first = Storage::connect(&format!("turso:{}", path.display()), Arc::new(StubPlugin))
        .await
        .unwrap();
    let second = Storage::connect(&format!("turso:{}", path.display()), Arc::new(StubPlugin))
        .await
        .unwrap();
    let (first_result, second_result) = tokio::join!(first.migrate(), second.migrate());
    first_result.expect("first concurrent migration");
    second_result.expect("second concurrent migration");
}

#[tokio::test]
async fn init_is_idempotent_via_on_conflict() {
    let storage = memory_storage().await;

    let (repo, main) = storage.init_repository("demo").await.unwrap();
    assert_eq!(main.name, "main");
    assert_eq!(main.base_position, 0);
    assert!(main.base_branch_id.is_none());

    // ON CONFLICT DO UPDATE paths: re-init yields the same repository and main branch.
    let (repo_again, main_again) = storage.init_repository("demo").await.unwrap();
    assert_eq!(repo.id, repo_again.id);
    assert_eq!(main.id, main_again.id);
}

#[tokio::test]
async fn append_allocates_sequential_positions() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let changeset = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "human")
        .await
        .unwrap();

    let first = storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("alpha"))
        .await
        .unwrap();
    let second = storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("beta"))
        .await
        .unwrap();
    let batched = storage
        .append_operations(
            repo.id,
            main.id,
            changeset.id,
            vec![create_symbol("gamma"), create_symbol("delta")],
        )
        .await
        .unwrap();
    assert_eq!(first.position, 1);
    assert_eq!(second.position, 2);
    assert_eq!(batched[0].position, 3);
    assert_eq!(batched[1].position, 4);

    let own = storage.list_own_operations(main.id, None).await.unwrap();
    assert_eq!(own.len(), 4);
    assert_eq!(own[0].position, 1);
    assert_eq!(own[1].position, 2);
    assert_eq!(own[2].position, 3);
    assert_eq!(own[3].position, 4);

    // payload round-tripped through TEXT and back into a typed Operation.
    assert!(matches!(
        own[0].operation,
        Operation::CreateSymbol { ref name, .. } if name == "alpha"
    ));

    let rejected = storage
        .append_operations_if_branch_position(
            repo.id,
            main.id,
            changeset.id,
            3,
            vec![create_symbol("epsilon")],
        )
        .await;
    assert!(rejected.is_err());
    assert_eq!(
        storage
            .list_own_operations(main.id, None)
            .await
            .unwrap()
            .len(),
        4
    );

    let checked = storage
        .append_operations_if_branch_position(
            repo.id,
            main.id,
            changeset.id,
            4,
            vec![create_symbol("epsilon")],
        )
        .await
        .unwrap();
    assert_eq!(checked[0].position, 5);
}

#[tokio::test]
async fn branch_fork_point_is_immutable_and_inherited() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let main_cs = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "human")
        .await
        .unwrap();
    storage
        .append_operation(repo.id, main.id, main_cs.id, create_symbol("alpha"))
        .await
        .unwrap();
    storage
        .append_operation(repo.id, main.id, main_cs.id, create_symbol("beta"))
        .await
        .unwrap();

    let feature = storage
        .create_branch(repo.id, "feature", "main")
        .await
        .unwrap();
    assert_eq!(feature.base_position, 2);
    assert_eq!(feature.base_branch_id, Some(main.id));

    // Re-creating an existing branch (ON CONFLICT DO NOTHING) returns the original, unchanged.
    storage
        .append_operation(repo.id, main.id, main_cs.id, create_symbol("gamma"))
        .await
        .unwrap();
    let feature_again = storage
        .create_branch(repo.id, "feature", "main")
        .await
        .unwrap();
    assert_eq!(feature.id, feature_again.id);
    assert_eq!(feature_again.base_position, 2);

    // A commit on feature stacks on top of the inherited base history.
    let feature_cs = storage
        .create_changeset(repo.id, task.id, feature.id, "work", "agent")
        .await
        .unwrap();
    let own = storage
        .append_operation(repo.id, feature.id, feature_cs.id, create_symbol("delta"))
        .await
        .unwrap();
    assert_eq!(own.position, 1);
    let collected = storage
        .collect_branch_operations(feature.id, None)
        .await
        .unwrap();
    assert_eq!(collected.len(), 3); // 2 inherited from the fork point + 1 own

    let branches = storage.list_branches(repo.id).await.unwrap();
    assert_eq!(branches.len(), 2);
}

#[tokio::test]
async fn materialize_round_trips_the_graph_cache() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let changeset = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "human")
        .await
        .unwrap();
    storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("alpha"))
        .await
        .unwrap();

    let miss = storage.materialize_branch("demo", "main").await.unwrap();
    assert_eq!(miss.cache_status, CacheStatus::Miss);
    assert_eq!(miss.operations.len(), 1);
    let symbol_count = miss.graph.symbols.len();

    // Second materialization hits the cache and deserializes the graph back out of TEXT JSON.
    let hit = storage.materialize_branch("demo", "main").await.unwrap();
    assert_eq!(hit.cache_status, CacheStatus::Hit);
    assert_eq!(hit.graph.symbols.len(), symbol_count);
    assert_eq!(hit.materialization.checkpoint_operations, 1);
    assert_eq!(hit.materialization.replayed_operations, 0);
    assert!(hit.materialization.prefix_fingerprint_verified);

    let _ = repo;
}

#[tokio::test]
async fn materialization_reuses_a_verified_large_prefix_and_replays_only_the_suffix() {
    const PREFIX_OPERATIONS: usize = 512;

    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("incremental").await.unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let changeset = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "agent")
        .await
        .unwrap();
    storage
        .append_operations(
            repo.id,
            main.id,
            changeset.id,
            (0..PREFIX_OPERATIONS)
                .map(|index| create_symbol(&format!("symbol_{index}")))
                .collect(),
        )
        .await
        .unwrap();

    let initial = storage
        .materialize_branch("incremental", "main")
        .await
        .unwrap();
    assert_eq!(initial.cache_status, CacheStatus::Miss);
    assert_eq!(
        initial.materialization.replayed_operations,
        PREFIX_OPERATIONS as i64
    );

    storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("suffix"))
        .await
        .unwrap();
    let incremental = storage
        .materialize_branch("incremental", "main")
        .await
        .unwrap();
    assert_eq!(incremental.cache_status, CacheStatus::Incremental);
    assert_eq!(
        incremental.materialization.checkpoint_operations,
        PREFIX_OPERATIONS as i64
    );
    assert_eq!(incremental.materialization.replayed_operations, 1);
    assert!(incremental.materialization.prefix_fingerprint_verified);

    let full_replay = bonhomme_core::materialize(&incremental.operations).unwrap();
    assert_eq!(incremental.graph, full_replay);

    let exact = storage
        .materialize_branch("incremental", "main")
        .await
        .unwrap();
    assert_eq!(exact.cache_status, CacheStatus::Hit);
    assert_eq!(exact.materialization.replayed_operations, 0);

    let historical = storage
        .materialize_branch_graph_at_position(main.id, PREFIX_OPERATIONS as i64)
        .await
        .unwrap();
    assert_eq!(historical.cache_status, CacheStatus::Miss);
    assert_eq!(
        historical.materialization.checkpoint_rejection,
        Some(CheckpointRejection::AheadOfRequestedPosition)
    );
    assert_eq!(
        historical.graph,
        bonhomme_core::materialize(&historical.operations).unwrap()
    );
}

#[tokio::test]
async fn mismatched_checkpoint_prefix_is_rejected_and_full_replay_remains_exact() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("corrupt-cache").await.unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let changeset = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "agent")
        .await
        .unwrap();
    storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("stable"))
        .await
        .unwrap();
    storage
        .materialize_branch("corrupt-cache", "main")
        .await
        .unwrap();

    let checkpoint = storage
        .backend
        .latest_graph_checkpoint(main.id)
        .await
        .unwrap()
        .unwrap();
    storage
        .backend
        .store_graph_cache(
            repo.id,
            main.id,
            checkpoint.operation_count,
            "sha256:not-the-operation-prefix",
            checkpoint.graph,
            json!([]),
        )
        .await
        .unwrap();

    let recovered = storage
        .materialize_branch("corrupt-cache", "main")
        .await
        .unwrap();
    assert_eq!(recovered.cache_status, CacheStatus::Miss);
    assert_eq!(
        recovered.materialization.checkpoint_rejection,
        Some(CheckpointRejection::PrefixFingerprintMismatch)
    );
    assert_eq!(
        recovered.graph,
        bonhomme_core::materialize(&recovered.operations).unwrap()
    );
}

#[tokio::test]
async fn verified_prefix_is_reported_when_checkpoint_payload_is_invalid() {
    let storage = memory_storage().await;
    let (repo, main) = storage
        .init_repository("invalid-checkpoint-payload")
        .await
        .unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let changeset = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "agent")
        .await
        .unwrap();
    storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("stable"))
        .await
        .unwrap();
    storage
        .materialize_branch("invalid-checkpoint-payload", "main")
        .await
        .unwrap();

    let checkpoint = storage
        .backend
        .latest_graph_checkpoint(main.id)
        .await
        .unwrap()
        .unwrap();
    storage
        .backend
        .store_graph_cache(
            repo.id,
            main.id,
            checkpoint.operation_count,
            &checkpoint.operation_fingerprint,
            json!({"not": "a checkpoint envelope"}),
            json!([]),
        )
        .await
        .unwrap();

    let recovered = storage
        .materialize_branch("invalid-checkpoint-payload", "main")
        .await
        .unwrap();
    assert_eq!(recovered.cache_status, CacheStatus::Miss);
    assert!(recovered.materialization.prefix_fingerprint_verified);
    assert_eq!(
        recovered.materialization.checkpoint_rejection,
        Some(CheckpointRejection::InvalidEncoding)
    );
}

#[tokio::test]
async fn slices_and_attachments_round_trip_json() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();

    let roots = vec![Uuid::new_v4(), Uuid::new_v4()];
    let slice = storage
        .create_slice(repo.id, main.id, 0, &roots)
        .await
        .unwrap();
    let fetched = storage.slice_by_id(slice.id).await.unwrap();
    assert_eq!(fetched.root_symbols, roots);

    let attachment = storage
        .add_attachment(
            repo.id,
            "branch",
            main.id,
            "note",
            json!({ "labels": ["a", "b"], "count": 3 }),
        )
        .await
        .unwrap();
    assert_eq!(attachment.payload["count"], 3);
    assert_eq!(attachment.payload["labels"][1], "b");
}

#[tokio::test]
async fn conflicts_round_trip_and_block_active_branch_deletion() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();
    let slice = storage
        .create_slice(repo.id, main.id, 0, &[])
        .await
        .unwrap();
    let proposed = vec![create_symbol("resolutionCandidate")];
    let conflict = storage
        .create_slice_apply_conflict(
            slice.id,
            &main,
            crate::ConflictPositions::new(0, 0),
            vec![MergeConflict {
                reason: "DUPLICATE_SYMBOL_NAME".to_string(),
                source_operation_id: Uuid::new_v4(),
                target_operation_id: None,
                symbol_id: None,
                detail: "both edits create the same symbol".to_string(),
            }],
            proposed.clone(),
            "agent-001",
        )
        .await
        .unwrap();

    assert_eq!(conflict.source_branch_id, Some(main.id));
    assert_eq!(conflict.target_branch_id, Some(main.id));
    assert_eq!(conflict.proposed_operations, proposed);
    assert!(conflict.state.is_active());

    let listed = storage.list_conflicts(repo.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, conflict.id);

    let delete = storage.delete_branch_by_name(repo.id, "main").await;
    assert!(delete.is_err());
    assert!(format!("{:#}", delete.unwrap_err()).contains("conflict"));

    let abandoned = storage.abandon_conflict(conflict.id).await.unwrap();
    assert_eq!(abandoned.state.to_string(), "abandoned");
    assert!(!abandoned.state.is_active());

    storage
        .delete_branch_by_name(repo.id, "main")
        .await
        .unwrap();
    let retained = storage.conflict_by_id(conflict.id).await.unwrap();
    assert_eq!(retained.source_branch_id, None);
    assert_eq!(retained.target_branch_id, None);
    assert_eq!(retained.slice_id, None);
    assert_eq!(retained.source_branch_name, "main");
    assert_eq!(retained.target_branch_name, "main");
}

#[tokio::test]
async fn source_file_snapshots_replace_and_reset_with_repository() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();
    let file_symbol_id = Uuid::new_v4();

    storage
        .replace_source_file_snapshots(
            repo.id,
            main.id,
            vec![PendingSourceFileSnapshot {
                path: "src/lib.ts".to_string(),
                content_hash: "sha256:a".to_string(),
                byte_len: 12,
                handler: "typescript".to_string(),
                file_symbol_id: Some(file_symbol_id),
                last_import_position: 4,
                importer_version: "test-v1".to_string(),
            }],
        )
        .await
        .unwrap();
    let snapshots = storage.list_source_file_snapshots(main.id).await.unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].path, "src/lib.ts");
    assert_eq!(snapshots[0].file_symbol_id, Some(file_symbol_id));

    storage
        .replace_source_file_snapshots(
            repo.id,
            main.id,
            vec![PendingSourceFileSnapshot {
                path: "src/main.ts".to_string(),
                content_hash: "sha256:b".to_string(),
                byte_len: 20,
                handler: "typescript".to_string(),
                file_symbol_id: None,
                last_import_position: 5,
                importer_version: "test-v2".to_string(),
            }],
        )
        .await
        .unwrap();
    let snapshots = storage.list_source_file_snapshots(main.id).await.unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].path, "src/main.ts");
    assert_eq!(snapshots[0].file_symbol_id, None);
    assert_eq!(snapshots[0].importer_version, "test-v2");

    let (_, fresh_main) = storage.reset_repository("demo").await.unwrap();
    assert!(
        storage
            .list_source_file_snapshots(fresh_main.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn reset_clears_all_child_rows() {
    let storage = memory_storage().await;
    let (repo, main) = storage.init_repository("demo").await.unwrap();
    let task = storage.create_task(repo.id, "seed").await.unwrap();
    let changeset = storage
        .create_changeset(repo.id, task.id, main.id, "seed", "human")
        .await
        .unwrap();
    storage
        .append_operation(repo.id, main.id, changeset.id, create_symbol("alpha"))
        .await
        .unwrap();
    storage
        .create_branch(repo.id, "feature", "main")
        .await
        .unwrap();

    let (fresh_repo, _) = storage.reset_repository("demo").await.unwrap();
    assert!(
        storage
            .list_operations(fresh_repo.id)
            .await
            .unwrap()
            .is_empty()
    );
    let branches = storage.list_branches(fresh_repo.id).await.unwrap();
    assert_eq!(branches.len(), 1, "only main should remain after reset");
}
