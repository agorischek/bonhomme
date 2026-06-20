use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, SymbolNode,
    ValidateFuture, metadata_string,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::ids::stable_uuid;

/// Structured-data handler for JSON. A JSON object decomposes into one symbol per top-level key, so
/// two branches editing different keys (`scripts` vs `dependencies` in a `package.json`) touch
/// different symbols and merge cleanly, while edits to the same key conflict. Identity is the
/// key-path — structural, stable, comment-free. Re-render is canonical (sorted keys, two-space
/// indent), so output is deterministic the way `gofmt` is, at the cost of not preserving original
/// byte-for-byte formatting (acceptable for JSON; see the plan's fidelity note).
///
/// A non-object top-level (array or scalar) stays a single file symbol whose body is the canonical
/// document — still validated as JSON, still merging at file granularity.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonHandler;

const KEY_KIND: &str = "json-key";

impl Handler for JsonHandler {
    fn name(&self) -> &str {
        "json"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        file.path.ends_with(".json")
    }
}

impl LanguagePlugin for JsonHandler {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut files = file_symbols(graph)
            .map(|file_symbol| RenderedFile {
                path: file_path(file_symbol),
                content: render_json_file(graph, file_symbol),
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
        let file_ids: Vec<Uuid> = if root_symbols.is_empty() {
            file_symbols(graph).map(|symbol| symbol.id).collect()
        } else {
            root_symbols
                .iter()
                .filter_map(|id| graph.symbols.get(id))
                .filter_map(|symbol| nearest_file_symbol(graph, symbol))
                .map(|symbol| symbol.id)
                .collect()
        };

        let mut seen = BTreeSet::new();
        let mut files = Vec::new();
        for id in file_ids {
            if seen.insert(id)
                && let Some(file_symbol) = graph.symbols.get(&id)
            {
                files.push(RenderedFile {
                    path: file_path(file_symbol),
                    content: render_json_file(graph, file_symbol),
                });
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));

        Slice {
            id: Uuid::new_v4(),
            base_revision,
            root_symbols,
            files,
        }
    }

    fn import(&self, files: &[RenderedFile]) -> Result<Vec<Operation>> {
        let mut operations = Vec::new();
        for file in files {
            operations.extend(import_json_file(&file.path, &file.content)?);
        }
        Ok(operations)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
        // Legacy two-blob path: parse the original into a throwaway graph and recover against it so
        // the same structural key-diff logic applies, with ids deterministically derived from paths.
        let mut base = SemanticGraph::default();
        for (index, operation) in self.import(original)?.into_iter().enumerate() {
            base.apply_operation(diff_op_id(index), &operation)?;
        }
        self.recover_operations(&base, &[], modified)
    }

    fn recover_operations(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        let base_files = base_files_by_path(base, scope);
        let mut creates = Vec::new();
        let mut updates = Vec::new();
        let mut deletes = Vec::new();
        let mut edited_paths = BTreeSet::new();

        for file in edited {
            edited_paths.insert(file.path.clone());
            match base_files.get(&file.path) {
                None => creates.extend(import_json_file(&file.path, &file.content)?),
                Some(file_symbol) => recover_json_file(
                    base,
                    file_symbol,
                    &file.path,
                    &file.content,
                    &mut creates,
                    &mut updates,
                    &mut deletes,
                )?,
            }
        }

        // A sliced JSON file absent from the edit is deleted — its key children first so the file
        // symbol has no children when its own delete applies.
        for (path, file_symbol) in &base_files {
            if !edited_paths.contains(path) {
                for child in base.children_of(file_symbol.id) {
                    deletes.push(Operation::DeleteSymbol {
                        symbol_id: child.id,
                    });
                }
                deletes.push(Operation::DeleteSymbol {
                    symbol_id: file_symbol.id,
                });
            }
        }

        // Creates parent-first, then updates, then deletes children-first: a globally valid order.
        let mut operations = creates;
        operations.extend(updates);
        operations.extend(deletes);
        Ok(operations)
    }

    fn read_source_tree(&self, root: &std::path::Path) -> Result<Vec<RenderedFile>> {
        Ok(bonhomme_core::read_source_files(root)?
            .into_iter()
            .filter(|file| self.claims(file))
            .collect())
    }

    fn validate<'a>(&'a self, files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        // Well-formedness: the rendered projection must parse as JSON. (Canonical re-serialization
        // makes this hold by construction, but validating closes the loop.)
        Box::pin(async move {
            for file in files {
                serde_json::from_str::<Value>(&file.content)
                    .with_context(|| format!("rendered {} is not valid JSON", file.path))?;
            }
            Ok(())
        })
    }
}

fn import_json_file(path: &str, content: &str) -> Result<Vec<Operation>> {
    let value: Value =
        serde_json::from_str(content).with_context(|| format!("{path} is not valid JSON"))?;
    let file_id = json_file_id(path);

    match value {
        Value::Object(map) => {
            let mut operations = vec![Operation::CreateSymbol {
                symbol_id: file_id,
                parent_id: None,
                kind: "file".to_string(),
                name: path.to_string(),
                body: None,
                metadata: json!({ "handler": "json", "path": path, "top": "object" }),
            }];
            for (key, value) in &map {
                operations.push(Operation::CreateSymbol {
                    symbol_id: json_key_id(path, key),
                    parent_id: Some(file_id),
                    kind: KEY_KIND.to_string(),
                    name: key.clone(),
                    body: Some(canonical_fragment(value)),
                    metadata: json!({}),
                });
            }
            Ok(operations)
        }
        other => Ok(vec![Operation::CreateSymbol {
            symbol_id: file_id,
            parent_id: None,
            kind: "file".to_string(),
            name: path.to_string(),
            body: Some(canonical_document(&other)),
            metadata: json!({ "handler": "json", "path": path, "top": "other" }),
        }]),
    }
}

#[allow(clippy::too_many_arguments)]
fn recover_json_file(
    base: &SemanticGraph,
    file_symbol: &SymbolNode,
    path: &str,
    content: &str,
    creates: &mut Vec<Operation>,
    updates: &mut Vec<Operation>,
    deletes: &mut Vec<Operation>,
) -> Result<()> {
    let value: Value =
        serde_json::from_str(content).with_context(|| format!("{path} is not valid JSON"))?;
    let base_children = base.children_of(file_symbol.id);
    let base_was_object = !base_children.is_empty();

    match value {
        Value::Object(map) => {
            // Key-level diff: identity is the key name, child of the file symbol.
            let base_by_key: BTreeMap<&str, &SymbolNode> = base_children
                .iter()
                .map(|child| (child.name.as_str(), *child))
                .collect();

            for (key, value) in &map {
                let fragment = canonical_fragment(value);
                match base_by_key.get(key.as_str()) {
                    Some(child) => {
                        if child.body.as_deref() != Some(fragment.as_str()) {
                            updates.push(Operation::UpdateSymbol {
                                symbol_id: child.id,
                                name: None,
                                body: Some(fragment),
                                metadata: None,
                            });
                        }
                    }
                    None => creates.push(Operation::CreateSymbol {
                        symbol_id: json_key_id(path, key),
                        parent_id: Some(file_symbol.id),
                        kind: KEY_KIND.to_string(),
                        name: key.clone(),
                        body: Some(fragment),
                        metadata: json!({}),
                    }),
                }
            }
            for child in &base_children {
                if !map.contains_key(&child.name) {
                    deletes.push(Operation::DeleteSymbol {
                        symbol_id: child.id,
                    });
                }
            }
        }
        other => {
            // Edited document is no longer an object: drop any key children and store the canonical
            // document on the file symbol (render prefers children, so the body governs once empty).
            if base_was_object {
                for child in &base_children {
                    deletes.push(Operation::DeleteSymbol {
                        symbol_id: child.id,
                    });
                }
            }
            let document = canonical_document(&other);
            if file_symbol.body.as_deref() != Some(document.as_str()) {
                updates.push(Operation::UpdateSymbol {
                    symbol_id: file_symbol.id,
                    name: None,
                    body: Some(document),
                    metadata: None,
                });
            }
        }
    }
    Ok(())
}

fn render_json_file(graph: &SemanticGraph, file_symbol: &SymbolNode) -> String {
    let children = graph.children_of(file_symbol.id);
    if children.is_empty() {
        // Scalar/array document, or an empty object — emit the stored body (canonical document).
        return file_symbol
            .body
            .clone()
            .unwrap_or_else(|| "{}\n".to_string());
    }
    let mut map = Map::new();
    for child in children {
        if child.kind == KEY_KIND {
            let value = child
                .body
                .as_deref()
                .and_then(|body| serde_json::from_str::<Value>(body).ok())
                .unwrap_or(Value::Null);
            map.insert(child.name.clone(), value);
        }
    }
    canonical_document(&Value::Object(map))
}

/// Pretty JSON with a trailing newline — the canonical form for a whole document.
fn canonical_document(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string());
    text.push('\n');
    text
}

/// Pretty JSON without a trailing newline — the canonical form for a stored key value.
fn canonical_fragment(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string())
}

fn file_symbols(graph: &SemanticGraph) -> impl Iterator<Item = &SymbolNode> {
    graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
}

fn base_files_by_path<'a>(
    base: &'a SemanticGraph,
    scope: &[Uuid],
) -> BTreeMap<String, &'a SymbolNode> {
    let ids: Vec<Uuid> = if scope.is_empty() {
        file_symbols(base).map(|symbol| symbol.id).collect()
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
        .map(|symbol| (file_path(symbol), symbol))
        .collect()
}

fn file_path(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone())
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

fn json_file_id(path: &str) -> Uuid {
    stable_uuid(&format!("json:file:{path}"))
}

fn json_key_id(path: &str, key: &str) -> Uuid {
    stable_uuid(&format!("json:key:{path}:{key}"))
}

fn diff_op_id(index: usize) -> Uuid {
    stable_uuid(&format!("json:diff-op:{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::graph_from;

    fn rendered(path: &str, content: &str) -> RenderedFile {
        RenderedFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn object_decomposes_into_one_symbol_per_top_level_key() {
        let operations = JsonHandler
            .import(&[rendered(
                "package.json",
                r#"{"name":"x","version":"1.0.0"}"#,
            )])
            .unwrap();
        let keys: BTreeSet<String> = operations
            .iter()
            .filter_map(|op| match op {
                Operation::CreateSymbol { kind, name, .. } if kind == KEY_KIND => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from(["name".to_string(), "version".to_string()])
        );
    }

    #[test]
    fn render_is_canonical_and_round_trips() {
        // Keys out of order and compact input render to sorted, pretty, newline-terminated JSON.
        let graph = graph_from(
            &JsonHandler
                .import(&[rendered("c.json", r#"{"b":2,"a":1}"#)])
                .unwrap(),
        );
        let rendered_files = JsonHandler.render(&graph);
        assert_eq!(rendered_files[0].content, "{\n  \"a\": 1,\n  \"b\": 2\n}\n");

        // Re-importing the canonical render yields the same graph shape (stable).
        let regraph = graph_from(&JsonHandler.import(&rendered_files).unwrap());
        assert_eq!(JsonHandler.render(&regraph), rendered_files);
    }

    #[test]
    fn recover_diffs_at_key_granularity() {
        let graph = graph_from(
            &JsonHandler
                .import(&[rendered("a.json", r#"{"keep":1,"change":2,"drop":3}"#)])
                .unwrap(),
        );
        let scope: Vec<Uuid> = graph.root_symbols().iter().map(|s| s.id).collect();
        let operations = JsonHandler
            .recover_operations(
                &graph,
                &scope,
                &[rendered("a.json", r#"{"keep":1,"change":9,"add":4}"#)],
            )
            .unwrap();

        // `keep` untouched; `change` updated; `add` created; `drop` deleted.
        assert_eq!(
            operations
                .iter()
                .filter(|op| matches!(op, Operation::UpdateSymbol { .. }))
                .count(),
            1
        );
        assert!(operations.iter().any(|op| matches!(op,
            Operation::CreateSymbol { name, .. } if name == "add")));
        assert!(
            operations
                .iter()
                .any(|op| matches!(op, Operation::DeleteSymbol { .. }))
        );
    }

    #[test]
    fn edits_to_different_keys_target_different_symbols() {
        // The merge payoff: two independent edits write disjoint symbols (clean merge), not the
        // whole file (conflict).
        let graph = graph_from(
            &JsonHandler
                .import(&[rendered("a.json", r#"{"x":1,"y":2}"#)])
                .unwrap(),
        );
        let scope: Vec<Uuid> = graph.root_symbols().iter().map(|s| s.id).collect();
        let edit_x = JsonHandler
            .recover_operations(&graph, &scope, &[rendered("a.json", r#"{"x":9,"y":2}"#)])
            .unwrap();
        let edit_y = JsonHandler
            .recover_operations(&graph, &scope, &[rendered("a.json", r#"{"x":1,"y":9}"#)])
            .unwrap();
        let symbol_of = |ops: &[Operation]| match &ops[0] {
            Operation::UpdateSymbol { symbol_id, .. } => *symbol_id,
            other => panic!("expected update, got {other:?}"),
        };
        assert_ne!(symbol_of(&edit_x), symbol_of(&edit_y));
    }

    #[test]
    fn non_object_top_level_is_a_single_file_symbol() {
        let operations = JsonHandler
            .import(&[rendered("list.json", "[1, 2, 3]")])
            .unwrap();
        assert!(matches!(
            operations.as_slice(),
            [Operation::CreateSymbol { kind, body: Some(_), .. }] if kind == "file"
        ));
        let graph = graph_from(&operations);
        assert_eq!(
            JsonHandler.render(&graph)[0].content,
            "[\n  1,\n  2,\n  3\n]\n"
        );
    }
}
