use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use super::{Handler, LanguagePlugin, RenderedFile, Slice, ValidateFuture, read_source_files};
use crate::core::{Operation, SemanticGraph, SymbolNode, metadata_string};

/// The universal fallback handler: any bytes round-trip, files survive, and merge happens at file
/// granularity. A file is a single symbol whose body is the whole file; identity is the path. There
/// is no validator (you cannot compile a README), so the merge gate simply does not apply — but
/// because the file is one symbol, two branches editing the same file still conflict at replay,
/// which keeps the blob tier on the safe side of "surface conflicts, do not guess".
#[derive(Clone, Copy, Debug, Default)]
pub struct BlobHandler;

impl Handler for BlobHandler {
    fn name(&self) -> &str {
        "blob"
    }

    fn claims(&self, _file: &RenderedFile) -> bool {
        // Terminal: the floor that always works.
        true
    }
}

impl LanguagePlugin for BlobHandler {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut files = blob_file_symbols(graph)
            .map(render_blob_file)
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        files
    }

    fn render_slice(
        &self,
        graph: &SemanticGraph,
        base_revision: String,
        root_symbols: Vec<Uuid>,
    ) -> Slice {
        let requested_files = if root_symbols.is_empty() {
            blob_file_symbols(graph).collect::<Vec<_>>()
        } else {
            root_symbols
                .iter()
                .filter_map(|symbol_id| graph.symbols.get(symbol_id))
                .filter_map(|symbol| nearest_file_symbol(graph, symbol))
                .collect::<Vec<_>>()
        };

        let mut seen = BTreeSet::new();
        let mut files = Vec::new();
        for file_symbol in requested_files {
            if seen.insert(file_symbol.id) {
                files.push(render_blob_file(file_symbol));
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));

        Slice {
            id: Uuid::new_v4(),
            base_revision,
            root_symbols,
            files,
        }
    }

    fn import(&self, files: &[RenderedFile]) -> Result<Vec<Operation>> {
        Ok(sorted_files(files)
            .into_iter()
            .map(create_file_op)
            .collect())
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        // Legacy two-blob path: identity derives from the path, since there is no graph to anchor
        // against. A changed file updates its body; a new file is created; a removed file is deleted.
        let original_by_path = original
            .iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let modified_by_path = modified
            .iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();

        let mut operations = Vec::new();
        for (path, original_file) in &original_by_path {
            match modified_by_path.get(path) {
                None => operations.push(Operation::DeleteSymbol {
                    symbol_id: blob_file_id(path),
                }),
                Some(modified_file) if modified_file.content != original_file.content => {
                    operations.push(Operation::UpdateSymbol {
                        symbol_id: blob_file_id(path),
                        name: None,
                        body: Some(modified_file.content.clone()),
                        metadata: None,
                    });
                }
                Some(_) => {}
            }
        }
        for (path, modified_file) in &modified_by_path {
            if !original_by_path.contains_key(path) {
                operations.push(create_file_op(modified_file));
            }
        }
        Ok(operations)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        // Identity is the path. The router hands the blob handler a subgraph of only its files, so
        // an empty scope means "every blob file"; a non-empty scope restricts to the sliced files.
        let base_files = blob_base_files(base, scope);
        let mut operations = Vec::new();
        let mut edited_paths = BTreeSet::new();

        for file in sorted_files(edited) {
            edited_paths.insert(file.path.clone());
            match base_files.get(&file.path) {
                Some((symbol_id, body)) => {
                    if body != &file.content {
                        operations.push(Operation::UpdateSymbol {
                            symbol_id: *symbol_id,
                            name: None,
                            body: Some(file.content.clone()),
                            metadata: None,
                        });
                    }
                }
                None => operations.push(create_file_op(file)),
            }
        }

        // A sliced file absent from the edit was deleted.
        for (path, (symbol_id, _)) in &base_files {
            if !edited_paths.contains(path) {
                operations.push(Operation::DeleteSymbol {
                    symbol_id: *symbol_id,
                });
            }
        }
        Ok(operations)
    }

    fn read_source_tree(&self, root: &Path) -> Result<Vec<RenderedFile>> {
        // The router owns whole-tree reading; standalone, the blob handler claims everything.
        Ok(read_source_files(root)?
            .into_iter()
            .filter(|file| self.claims(file))
            .collect())
    }

    fn validate<'a>(&'a self, _files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        // No validator applies to opaque bytes; the merge gate skips blob files.
        Box::pin(async { Ok(()) })
    }
}

/// The `CreateSymbol` for a blob file: one opaque symbol, body = the whole file, identity = path.
/// The body holds the content verbatim, including a base64 envelope for binary files, so render is a
/// straight passthrough; the `encoding` tag records binary-ness for transparency.
fn create_file_op(file: &RenderedFile) -> Operation {
    let metadata = if super::is_binary(&file.content) {
        json!({ "handler": "blob", "path": file.path, "encoding": "base64" })
    } else {
        json!({ "handler": "blob", "path": file.path })
    };
    Operation::CreateSymbol {
        symbol_id: blob_file_id(&file.path),
        parent_id: None,
        kind: "file".to_string(),
        // The full path (not the basename) is the name so two same-named files in different
        // directories — `package.json` and `demo/package.json` — do not collide on the
        // (parent, kind, name) sibling key the graph enforces.
        name: file.path.clone(),
        body: Some(file.content.clone()),
        metadata,
    }
}

fn render_blob_file(symbol: &SymbolNode) -> RenderedFile {
    RenderedFile {
        path: file_symbol_path(symbol),
        content: symbol.body.clone().unwrap_or_default(),
    }
}

fn blob_file_symbols(graph: &SemanticGraph) -> impl Iterator<Item = &SymbolNode> {
    graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
}

/// Map base blob file paths to `(symbol id, body)`. An empty scope is every file symbol in the
/// graph (the router passes a subgraph of only blob files); otherwise the nearest file of each
/// scope symbol.
fn blob_base_files(base: &SemanticGraph, scope: &[Uuid]) -> BTreeMap<String, (Uuid, String)> {
    let mut file_ids = BTreeSet::new();
    if scope.is_empty() {
        file_ids.extend(blob_file_symbols(base).map(|symbol| symbol.id));
    } else {
        for symbol_id in scope {
            if let Some(symbol) = base.symbols.get(symbol_id)
                && let Some(file) = nearest_file_symbol(base, symbol)
            {
                file_ids.insert(file.id);
            }
        }
    }

    file_ids
        .into_iter()
        .filter_map(|id| base.symbols.get(&id))
        .map(|symbol| {
            (
                file_symbol_path(symbol),
                (symbol.id, symbol.body.clone().unwrap_or_default()),
            )
        })
        .collect()
}

fn file_symbol_path(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone())
}

/// The stable symbol id for a blob file, keyed by its path so re-import is deterministic.
fn blob_file_id(path: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/blob/file:{path}").as_bytes(),
    )
}

fn sorted_files(files: &[RenderedFile]) -> Vec<&RenderedFile> {
    let mut sorted = files.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    sorted
}

fn nearest_file_symbol<'a>(
    graph: &'a SemanticGraph,
    symbol: &'a SymbolNode,
) -> Option<&'a SymbolNode> {
    let mut current = symbol;
    loop {
        if current.kind == "file" {
            return Some(current);
        }
        current = graph.symbols.get(&current.parent_id?)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::materialize;
    use crate::core::{Operation, OperationRecord};

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
        materialize(&records).expect("blob operations materialize")
    }

    #[test]
    fn import_render_round_trip_is_byte_stable() {
        let files = vec![
            rendered("README.md", "# Title\n\nbody text\n"),
            rendered("config/app.yaml", "key: value\n# comment\n"),
        ];
        let operations = BlobHandler.import(&files).unwrap();
        let graph = graph_from(&operations);
        let mut expected = files.clone();
        expected.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(BlobHandler.render(&graph), expected);
    }

    #[test]
    fn file_symbol_is_tagged_and_path_named() {
        let operations = BlobHandler
            .import(&[rendered("a/b/index.md", "x")])
            .unwrap();
        let Operation::CreateSymbol {
            kind,
            name,
            metadata,
            ..
        } = &operations[0]
        else {
            panic!("expected CreateSymbol");
        };
        assert_eq!(kind, "file");
        assert_eq!(name, "a/b/index.md");
        assert_eq!(metadata["handler"], "blob");
        assert_eq!(metadata["path"], "a/b/index.md");
    }

    #[test]
    fn same_basename_in_different_dirs_does_not_collide() {
        // The graph's duplicate-sibling invariant would reject two `(None, "file", "README.md")`
        // symbols; full-path names keep them distinct, so a real polyglot repo imports.
        let files = vec![rendered("a/README.md", "a"), rendered("b/README.md", "b")];
        let graph = graph_from(&BlobHandler.import(&files).unwrap());
        assert_eq!(graph.symbols.len(), 2);
    }

    #[test]
    fn recover_updates_only_changed_files() {
        let base = graph_from(&BlobHandler.import(&[rendered("notes.txt", "old")]).unwrap());
        let scope = base
            .root_symbols()
            .iter()
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>();

        let unchanged = BlobHandler
            .recover_operations(&base, &scope, &[rendered("notes.txt", "old")])
            .unwrap();
        assert!(unchanged.is_empty(), "a no-op edit yields no operations");

        let changed = BlobHandler
            .recover_operations(&base, &scope, &[rendered("notes.txt", "new")])
            .unwrap();
        assert!(matches!(
            changed.as_slice(),
            [Operation::UpdateSymbol { body: Some(body), .. }] if body == "new"
        ));
    }

    #[test]
    fn recover_creates_new_and_deletes_missing() {
        let base = graph_from(
            &BlobHandler
                .import(&[rendered("keep.txt", "k"), rendered("drop.txt", "d")])
                .unwrap(),
        );
        let scope = base
            .root_symbols()
            .iter()
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>();

        let operations = BlobHandler
            .recover_operations(
                &base,
                &scope,
                &[rendered("keep.txt", "k"), rendered("fresh.txt", "f")],
            )
            .unwrap();

        let creates = operations
            .iter()
            .filter(|op| matches!(op, Operation::CreateSymbol { name, .. } if name == "fresh.txt"))
            .count();
        let deletes = operations
            .iter()
            .filter(|op| matches!(op, Operation::DeleteSymbol { .. }))
            .count();
        assert_eq!(creates, 1, "the new file is created");
        assert_eq!(deletes, 1, "the missing file is deleted");
    }

    #[test]
    fn diff_updates_creates_and_deletes_by_path() {
        let original = vec![rendered("a.txt", "1"), rendered("gone.txt", "x")];
        let modified = vec![rendered("a.txt", "2"), rendered("new.txt", "y")];
        let operations = BlobHandler.diff(&original, &modified).unwrap();

        assert!(operations.iter().any(|op| matches!(op,
            Operation::UpdateSymbol { symbol_id, .. } if *symbol_id == blob_file_id("a.txt"))));
        assert!(operations.iter().any(|op| matches!(op,
            Operation::CreateSymbol { name, .. } if name == "new.txt")));
        assert!(operations.iter().any(|op| matches!(op,
            Operation::DeleteSymbol { symbol_id } if *symbol_id == blob_file_id("gone.txt"))));
    }

    #[test]
    fn binary_file_round_trips_and_is_tagged() {
        use crate::lang::{decode_binary, encode_binary};
        let bytes = [0xFFu8, 0x00, 0x10, 0xAB, 0xCD, 0x00];
        let file = rendered("logo.png", &encode_binary(&bytes));

        let operations = BlobHandler.import(std::slice::from_ref(&file)).unwrap();
        let Operation::CreateSymbol { metadata, .. } = &operations[0] else {
            panic!("expected CreateSymbol");
        };
        assert_eq!(metadata["encoding"], "base64", "binary blobs are tagged");

        // Through the graph and back out, the rendered content decodes to the original bytes.
        let graph = graph_from(&operations);
        let rendered_back = BlobHandler.render(&graph);
        assert_eq!(
            decode_binary(&rendered_back[0].content).as_deref(),
            Some(&bytes[..])
        );
    }
}
