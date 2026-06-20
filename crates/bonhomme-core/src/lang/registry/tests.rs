use super::*;
use crate::core::{OperationRecord, materialize};
use serde_json::json;

/// A non-terminal fake handler that claims `.up` files and renders with an `UP:` prefix, so a
/// test can observe *which* handler rendered a file. Mirrors the blob handler's shape.
struct UpperHandler;

impl Handler for UpperHandler {
    fn name(&self) -> &str {
        "upper"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        file.path.ends_with(".up")
    }
}

impl LanguagePlugin for UpperHandler {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut files = file_symbols(graph)
            .into_iter()
            .map(|symbol| RenderedFile {
                path: file_symbol_path(symbol),
                content: format!("UP:{}", symbol.body.clone().unwrap_or_default()),
            })
            .collect::<Vec<_>>();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    fn render_slice(
        &self,
        graph: &SemanticGraph,
        base_revision: String,
        root_symbols: Vec<Uuid>,
    ) -> Slice {
        Slice {
            id: Uuid::new_v4(),
            base_revision,
            root_symbols,
            files: self.render(graph),
        }
    }

    fn import(&self, files: &[RenderedFile]) -> Result<Vec<Operation>> {
        // `BOOM` stands in for unparseable content, so a test can exercise degrade-to-blob.
        if files.iter().any(|file| file.content.contains("BOOM")) {
            anyhow::bail!("upper handler cannot parse BOOM");
        }
        Ok(files.iter().map(upper_create).collect())
    }

    fn diff(&self, _: &[RenderedFile], _: &[RenderedFile]) -> Result<Vec<Operation>> {
        Ok(Vec::new())
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        let base_files = upper_base_files(base, scope);
        let mut operations = Vec::new();
        let mut seen = BTreeSet::new();
        for file in edited {
            seen.insert(file.path.clone());
            match base_files.get(&file.path) {
                Some((id, body)) if body != &file.content => {
                    operations.push(Operation::UpdateSymbol {
                        symbol_id: *id,
                        name: None,
                        body: Some(file.content.clone()),
                        metadata: None,
                    })
                }
                Some(_) => {}
                None => operations.push(upper_create(file)),
            }
        }
        for (path, (id, _)) in &base_files {
            if !seen.contains(path) {
                operations.push(Operation::DeleteSymbol { symbol_id: *id });
            }
        }
        Ok(operations)
    }

    fn read_source_tree(&self, _: &std::path::Path) -> Result<Vec<RenderedFile>> {
        Ok(Vec::new())
    }

    fn validate<'a>(&'a self, _: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn upper_id(path: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("upper:{path}").as_bytes())
}

fn upper_create(file: &RenderedFile) -> Operation {
    Operation::CreateSymbol {
        symbol_id: upper_id(&file.path),
        parent_id: None,
        kind: "file".to_string(),
        name: file.path.clone(),
        body: Some(file.content.clone()),
        metadata: json!({ "handler": "upper", "path": file.path }),
    }
}

fn upper_base_files(base: &SemanticGraph, scope: &[Uuid]) -> BTreeMap<String, (Uuid, String)> {
    let ids: Vec<Uuid> = if scope.is_empty() {
        file_symbols(base).iter().map(|symbol| symbol.id).collect()
    } else {
        scope
            .iter()
            .filter_map(|id| base.symbols.get(id))
            .filter_map(|symbol| nearest_file_symbol(base, symbol))
            .map(|symbol| symbol.id)
            .collect()
    };
    ids.into_iter()
        .filter_map(|id| base.symbols.get(&id))
        .map(|symbol| {
            (
                file_symbol_path(symbol),
                (symbol.id, symbol.body.clone().unwrap_or_default()),
            )
        })
        .collect()
}

fn registry() -> HandlerRegistry {
    HandlerRegistry::new(vec![
        Arc::new(UpperHandler),
        Arc::new(super::super::BlobHandler),
    ])
}

fn rendered(path: &str, content: &str) -> RenderedFile {
    RenderedFile {
        path: path.to_string(),
        content: content.to_string(),
    }
}

fn graph_from(operations: &[Operation]) -> SemanticGraph {
    let records = operations
        .iter()
        .enumerate()
        .map(|(index, operation)| OperationRecord {
            id: Uuid::new_v4(),
            repository_id: Uuid::nil(),
            branch_id: Uuid::nil(),
            changeset_id: Uuid::nil(),
            position: index as i64 + 1,
            operation: operation.clone(),
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        })
        .collect::<Vec<_>>();
    materialize(&records).expect("operations materialize")
}

#[test]
fn import_and_render_route_each_file_to_its_handler() {
    let registry = registry();
    let operations = registry
        .import(&[rendered("a.up", "hi"), rendered("b.txt", "yo")])
        .unwrap();
    let graph = graph_from(&operations);
    assert_eq!(
        registry.render(&graph),
        vec![rendered("a.up", "UP:hi"), rendered("b.txt", "yo")]
    );
}

#[test]
fn claims_first_match_wins_and_blob_is_terminal() {
    let registry = registry();
    let operations = registry
        .import(&[rendered("x.up", "u"), rendered("y.md", "m")])
        .unwrap();
    let tags: BTreeMap<String, String> = operations
        .iter()
        .filter_map(|op| match op {
            Operation::CreateSymbol { name, metadata, .. } => Some((
                name.clone(),
                metadata["handler"].as_str().unwrap().to_string(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(tags["x.up"], "upper");
    assert_eq!(tags["y.md"], "blob");
}

#[test]
fn untagged_file_symbol_renders_by_claims() {
    let create = Operation::CreateSymbol {
        symbol_id: Uuid::new_v4(),
        parent_id: None,
        kind: "file".to_string(),
        name: "legacy.up".to_string(),
        body: Some("z".to_string()),
        metadata: json!({ "path": "legacy.up" }),
    };
    let graph = graph_from(&[create]);
    assert_eq!(
        registry().render(&graph),
        vec![rendered("legacy.up", "UP:z")]
    );
}

#[test]
fn focused_recover_does_not_delete_files_outside_scope() {
    let registry = registry();
    let graph = graph_from(
        &registry
            .import(&[
                rendered("a.up", "1"),
                rendered("keep.txt", "k"),
                rendered("drop.txt", "d"),
            ])
            .unwrap(),
    );
    let operations = registry
        .recover_operations(&graph, &[upper_id("a.up")], &[rendered("a.up", "2")])
        .unwrap();
    assert!(
        !operations
            .iter()
            .any(|op| matches!(op, Operation::DeleteSymbol { .. })),
        "out-of-scope blob files must not be deleted, got {operations:?}"
    );
    assert!(matches!(
        operations.as_slice(),
        [Operation::UpdateSymbol { body: Some(body), .. }] if body == "2"
    ));
}

#[test]
fn whole_repo_recover_deletes_missing_across_handlers() {
    let registry = registry();
    let graph = graph_from(
        &registry
            .import(&[rendered("a.up", "1"), rendered("drop.txt", "d")])
            .unwrap(),
    );
    let operations = registry
        .recover_operations(&graph, &[], &[rendered("a.up", "1")])
        .unwrap();
    assert_eq!(operations.len(), 1, "got {operations:?}");
    assert!(matches!(
        operations.as_slice(),
        [Operation::DeleteSymbol { .. }]
    ));
}

#[test]
fn read_source_files_walks_all_files_skipping_ignored_and_binary() {
    let root =
        std::env::temp_dir().join(format!("bonhomme-walk-{}-{}", std::process::id(), line!()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    std::fs::write(root.join("src/a.up"), "x").unwrap();
    std::fs::write(root.join("README.md"), "y").unwrap();
    std::fs::write(root.join("node_modules/ignored.txt"), "z").unwrap();
    std::fs::write(root.join("logo.bin"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();

    let files = read_source_files(&root).unwrap();
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let logo_is_binary = files
        .iter()
        .find(|file| file.path == "logo.bin")
        .map(|file| crate::is_binary(&file.content));
    let _ = std::fs::remove_dir_all(&root);

    // Every extension is read, sorted; ignored directories are skipped; the non-UTF-8 file comes
    // in as a base64 binary envelope rather than being dropped.
    assert_eq!(paths, vec!["README.md", "logo.bin", "src/a.up"]);
    assert_eq!(logo_is_binary, Some(true));
}

#[test]
fn unparseable_file_degrades_to_blob_by_default() {
    let registry = registry();
    // `a.up` parses; `b.up` is "broken" — only the broken file should fall back to blob.
    let operations = registry
        .import(&[rendered("a.up", "ok"), rendered("b.up", "BOOM")])
        .unwrap();
    let tags: BTreeMap<String, String> = operations
        .iter()
        .filter_map(|op| match op {
            Operation::CreateSymbol { name, metadata, .. } => Some((
                name.clone(),
                metadata["handler"].as_str().unwrap().to_string(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(tags["a.up"], "upper", "the good file keeps its handler");
    assert_eq!(tags["b.up"], "blob", "the broken file degrades to blob");
}

#[test]
fn rejecting_policy_propagates_the_parse_error() {
    let registry = HandlerRegistry::new(vec![
        Arc::new(UpperHandler),
        Arc::new(super::super::BlobHandler),
    ])
    .rejecting_parse_errors();
    assert!(registry.import(&[rendered("b.up", "BOOM")]).is_err());
}

#[test]
fn handler_breakdown_counts_files_and_surfaces_degradation() {
    let registry = registry();
    let graph = graph_from(
        &registry
            .import(&[
                rendered("a.up", "ok"),
                rendered("b.txt", "x"),
                rendered("c.up", "BOOM"),
            ])
            .unwrap(),
    );
    let breakdown = registry.handler_breakdown(&graph);
    assert_eq!(breakdown.get("upper"), Some(&1));
    // The plain blob file plus the degraded `c.up` both count as opaque blobs.
    assert_eq!(breakdown.get("blob"), Some(&2));
}
