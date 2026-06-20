//! End-to-end coverage of the embedded Turso backend, exercised through the public `Storage` API
//! against an in-memory database (no server required). This proves the SQLite-dialect SQL, the
//! TEXT/JSON round-tripping, the single-statement position allocation, the immutable fork point,
//! the graph cache, and the explicit-cascade reset all behave like the Postgres backend.

use std::path::Path;
use std::sync::Arc;

use bonhomme_core::{
    LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, ValidateFuture,
};
use serde_json::json;
use uuid::Uuid;

use crate::{CacheStatus, Storage};

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

    let _ = repo;
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
