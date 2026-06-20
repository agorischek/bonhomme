use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, SymbolNode,
    ValidateFuture, metadata_string,
};
use serde_json::json;
use uuid::Uuid;

use crate::ids::stable_uuid;

/// Span-preserving handler for YAML. Like TOML, YAML is comment-heavy and would lose its layout
/// under canonical re-serialization, so it is split into sections by *top-level* (column-zero)
/// mapping keys, each section's body being the exact source span from its key line through the line
/// before the next top-level key (its indented block comes along). Two branches editing different
/// top-level keys merge; edits to the same key conflict. Identity is the key.
///
/// Sections tile the file contiguously, so render is byte-identical no matter where a boundary
/// lands — the column-zero scanner only affects merge granularity, never fidelity. Block scalars and
/// nested mappings are naturally handled because their content is indented (so never column-zero);
/// genuinely exotic YAML (multi-line flow collections at column zero) merely merges coarsely.
#[derive(Clone, Copy, Debug, Default)]
pub struct YamlHandler;

const NODE_KIND: &str = "node";

impl Handler for YamlHandler {
    fn name(&self) -> &str {
        "yaml"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        file.path.ends_with(".yaml") || file.path.ends_with(".yml")
    }
}

impl LanguagePlugin for YamlHandler {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut files = file_symbols(graph)
            .map(|file_symbol| RenderedFile {
                path: file_path(file_symbol),
                content: render_file(graph, file_symbol),
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
                    content: render_file(graph, file_symbol),
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
            operations.extend(import_file(&file.path, &file.content));
        }
        Ok(operations)
    }

    fn diff(&self, original: &[RenderedFile], modified: &[RenderedFile]) -> Result<Vec<Operation>> {
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
                None => creates.extend(import_file(&file.path, &file.content)),
                Some(file_symbol) => recover_file(
                    base,
                    file_symbol,
                    &file.path,
                    &file.content,
                    &mut creates,
                    &mut updates,
                    &mut deletes,
                ),
            }
        }

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

    fn validate<'a>(&'a self, _files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

struct Section {
    name: String,
    body: String,
}

struct Parsed {
    preamble: String,
    sections: Vec<Section>,
}

/// Split YAML into the preamble (anything before the first top-level key) and one section per
/// column-zero mapping key, spanning from that key line to the next top-level key.
fn parse_yaml(content: &str) -> Parsed {
    let starts = top_level_starts(content);
    if starts.is_empty() {
        return Parsed {
            preamble: content.to_string(),
            sections: Vec::new(),
        };
    }

    let preamble = content[..starts[0].0].to_string();
    let mut sections = Vec::new();
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (index, (start, key)) in starts.iter().enumerate() {
        let end = starts
            .get(index + 1)
            .map(|next| next.0)
            .unwrap_or(content.len());
        let body = content[*start..end].to_string();

        let count = name_counts.entry(key.clone()).or_insert(0);
        *count += 1;
        let name = if *count == 1 {
            key.clone()
        } else {
            format!("{key} ({count})")
        };

        sections.push(Section { name, body });
    }

    Parsed { preamble, sections }
}

/// Byte offset and key of each top-level node: a column-zero line that is not blank, a comment, or a
/// document marker. The key is the text before the first `:` (or the whole trimmed line for a
/// sequence item / bare scalar).
fn top_level_starts(content: &str) -> Vec<(usize, String)> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let first = line.as_bytes().first().copied();
        let is_top_level = matches!(first, Some(c) if c != b' ' && c != b'\t' && c != b'#' && c != b'\n' && c != b'\r')
            && !line.starts_with("---")
            && !line.starts_with("...");
        if is_top_level {
            starts.push((line_start, top_level_key(line)));
        }
    }
    starts
}

fn top_level_key(line: &str) -> String {
    let trimmed = line.trim_end();
    match trimmed.split_once(':') {
        Some((key, _)) => key.trim().to_string(),
        None => trimmed.trim().to_string(),
    }
}

fn import_file(path: &str, content: &str) -> Vec<Operation> {
    let parsed = parse_yaml(content);
    let file_id = file_id(path);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: file_id,
        parent_id: None,
        kind: "file".to_string(),
        name: path.to_string(),
        body: None,
        metadata: file_metadata(path, &parsed.preamble),
    }];
    for section in &parsed.sections {
        operations.push(section_create(path, file_id, section));
    }
    operations
}

fn recover_file(
    base: &SemanticGraph,
    file_symbol: &SymbolNode,
    path: &str,
    content: &str,
    creates: &mut Vec<Operation>,
    updates: &mut Vec<Operation>,
    deletes: &mut Vec<Operation>,
) {
    let parsed = parse_yaml(content);

    if metadata_string(&file_symbol.metadata, "preamble").unwrap_or_default() != parsed.preamble {
        updates.push(Operation::UpdateSymbol {
            symbol_id: file_symbol.id,
            name: None,
            body: None,
            metadata: Some(file_metadata(path, &parsed.preamble)),
        });
    }

    let base_by_name: BTreeMap<&str, &SymbolNode> = base
        .children_of(file_symbol.id)
        .into_iter()
        .filter(|child| child.kind == NODE_KIND)
        .map(|child| (child.name.as_str(), child))
        .collect();
    let edited_names: BTreeSet<&str> = parsed.sections.iter().map(|s| s.name.as_str()).collect();

    for section in &parsed.sections {
        match base_by_name.get(section.name.as_str()) {
            Some(child) => {
                if child.body.as_deref() != Some(section.body.as_str()) {
                    updates.push(Operation::UpdateSymbol {
                        symbol_id: child.id,
                        name: None,
                        body: Some(section.body.clone()),
                        metadata: None,
                    });
                }
            }
            None => creates.push(section_create(path, file_symbol.id, section)),
        }
    }
    for (name, child) in &base_by_name {
        if !edited_names.contains(name) {
            deletes.push(Operation::DeleteSymbol {
                symbol_id: child.id,
            });
        }
    }
}

fn render_file(graph: &SemanticGraph, file_symbol: &SymbolNode) -> String {
    let mut content = metadata_string(&file_symbol.metadata, "preamble").unwrap_or_default();
    for child in graph.children_of(file_symbol.id) {
        if child.kind == NODE_KIND {
            content.push_str(child.body.as_deref().unwrap_or(""));
        }
    }
    content
}

fn section_create(path: &str, file_id: Uuid, section: &Section) -> Operation {
    Operation::CreateSymbol {
        symbol_id: section_id(path, &section.name),
        parent_id: Some(file_id),
        kind: NODE_KIND.to_string(),
        name: section.name.clone(),
        body: Some(section.body.clone()),
        metadata: json!({}),
    }
}

fn file_metadata(path: &str, preamble: &str) -> serde_json::Value {
    json!({ "handler": "yaml", "path": path, "preamble": preamble })
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

fn file_id(path: &str) -> Uuid {
    stable_uuid(&format!("yaml:file:{path}"))
}

fn section_id(path: &str, name: &str) -> Uuid {
    stable_uuid(&format!("yaml:section:{path}:{name}"))
}

fn diff_op_id(index: usize) -> Uuid {
    stable_uuid(&format!("yaml:diff-op:{index}"))
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

    const DOC: &str = "# config\nname: demo\n\nbuild:\n  os: linux\n  steps:\n    - run: make\n\ndeploy:\n  region: us-east\n";

    #[test]
    fn import_render_is_byte_identical_preserving_comments() {
        let graph = graph_from(&YamlHandler.import(&[rendered("ci.yaml", DOC)]).unwrap());
        assert_eq!(YamlHandler.render(&graph)[0].content, DOC);
    }

    #[test]
    fn top_level_keys_become_sections_indented_blocks_attached() {
        let operations = YamlHandler.import(&[rendered("ci.yaml", DOC)]).unwrap();
        let names: BTreeSet<String> = operations
            .iter()
            .filter_map(|op| match op {
                Operation::CreateSymbol { kind, name, .. } if kind == NODE_KIND => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        // `name`, `build`, `deploy` are top-level; `os`/`steps`/`region` are indented children,
        // folded into their parent section, not separate symbols.
        assert_eq!(
            names,
            BTreeSet::from([
                "name".to_string(),
                "build".to_string(),
                "deploy".to_string()
            ])
        );
    }

    #[test]
    fn editing_one_top_level_key_targets_only_that_symbol() {
        let graph = graph_from(&YamlHandler.import(&[rendered("ci.yaml", DOC)]).unwrap());
        let scope: Vec<Uuid> = graph.root_symbols().iter().map(|s| s.id).collect();
        let edited = DOC.replace("region: us-east", "region: eu-west");
        let operations = YamlHandler
            .recover_operations(&graph, &scope, &[rendered("ci.yaml", &edited)])
            .unwrap();
        assert_eq!(operations.len(), 1, "only `deploy` changes: {operations:?}");
        assert!(matches!(
            operations.as_slice(),
            [Operation::UpdateSymbol { body: Some(_), .. }]
        ));
    }

    #[test]
    fn comment_only_or_indented_lines_are_not_top_level() {
        // A block scalar's deeper content begins with `#` after indentation but is never column-zero,
        // so it stays inside its key's section and the file round-trips.
        let doc = "script: |\n  # not a top-level comment\n  echo hi\nother: 1\n";
        let graph = graph_from(&YamlHandler.import(&[rendered("a.yaml", doc)]).unwrap());
        let nodes = graph
            .symbols
            .values()
            .filter(|s| s.kind == NODE_KIND)
            .count();
        assert_eq!(nodes, 2, "script and other");
        assert_eq!(YamlHandler.render(&graph)[0].content, doc);
    }
}
