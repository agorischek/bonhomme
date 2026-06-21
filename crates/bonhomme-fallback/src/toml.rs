use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, SymbolNode,
    ValidateFuture, metadata_string,
};
use serde_json::json;
use uuid::Uuid;

use crate::ids::stable_uuid;

/// Span-preserving handler for TOML. Comment-heavy config formats lose their comments and layout
/// under canonical re-serialization (the JSON handler's approach), so TOML is instead split by
/// `[table]` / `[[array]]` headers into sections whose bodies are the *exact* source spans. Two
/// branches editing different tables (`[dependencies]` vs `[dev-dependencies]`) merge; edits to the
/// same table conflict. Identity is the table path.
///
/// Because the sections tile the file contiguously (preamble + each header-to-next-header span),
/// render is concatenation and is byte-identical regardless of where the split lands — so the header
/// scanner only affects merge granularity, never fidelity. YAML, being whitespace-significant and
/// far harder to span-split safely, stays at the blob tier until a format-preserving parser is wired
/// in (per the plan's fidelity note).
#[derive(Clone, Copy, Debug, Default)]
pub struct TomlHandler;

const TABLE_KIND: &str = "table";
const ARRAY_KIND: &str = "table-array";

impl Handler for TomlHandler {
    fn name(&self) -> &str {
        "toml"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        file.path.ends_with(".toml")
    }
}

impl LanguagePlugin for TomlHandler {
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
        let desired_ops = self.import(edited)?;
        crate::recover_from_imported_operations(base, scope, edited, &desired_ops)
    }

    fn read_source_tree(&self, root: &std::path::Path) -> Result<Vec<RenderedFile>> {
        Ok(bonhomme_core::read_source_files(root)?
            .into_iter()
            .filter(|file| self.claims(file))
            .collect())
    }

    fn validate<'a>(&'a self, _files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        // Span-preserving render cannot break well-formedness (it is the original bytes re-tiled), so
        // like the markdown/tree-sitter tiers this passes the merge gate opaquely.
        Box::pin(async { Ok(()) })
    }
}

struct Section {
    kind: String,
    name: String,
    body: String,
}

struct Parsed {
    preamble: String,
    sections: Vec<Section>,
}

/// Split TOML into the preamble (anything before the first table header — top-level keys and
/// comments) and one section per `[table]` / `[[array]]`, each spanning from its header to the next.
fn parse_toml(content: &str) -> Parsed {
    let headers = table_headers(content);
    if headers.is_empty() {
        return Parsed {
            preamble: content.to_string(),
            sections: Vec::new(),
        };
    }

    let preamble = content[..headers[0].start].to_string();
    let mut sections = Vec::new();
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (index, header) in headers.iter().enumerate() {
        let end = headers
            .get(index + 1)
            .map(|next| next.start)
            .unwrap_or(content.len());
        let body = content[header.start..end].to_string();

        // Array-of-tables and duplicate paths repeat; suffix repeats so the sibling key stays unique.
        let dedup_key = format!("{}\u{0}{}", header.kind, header.name);
        let count = name_counts.entry(dedup_key).or_insert(0);
        *count += 1;
        let name = if *count == 1 {
            header.name.clone()
        } else {
            format!("{} ({count})", header.name)
        };

        sections.push(Section {
            kind: header.kind.clone(),
            name,
            body,
        });
    }

    Parsed { preamble, sections }
}

struct Header {
    start: usize,
    kind: String,
    name: String,
}

/// Find genuine top-level table headers: a line whose first non-whitespace character is `[`, while
/// not inside a multi-line string or an unclosed multi-line array/inline-table. Misjudging only
/// shifts a merge boundary; it never corrupts content, since render re-tiles the original spans.
fn table_headers(content: &str) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut in_basic_multiline = false;
    let mut in_literal_multiline = false;
    let mut depth: i32 = 0;
    let mut offset = 0;

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let at_top_level = !in_basic_multiline && !in_literal_multiline && depth == 0;
        if at_top_level {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[')
                && let Some((kind, name)) = parse_header(trimmed)
            {
                headers.push(Header {
                    start: line_start,
                    kind,
                    name,
                });
            }
        }

        scan_line(
            line,
            &mut in_basic_multiline,
            &mut in_literal_multiline,
            &mut depth,
        );
    }

    headers
}

/// Parse a trimmed header line `[a.b]` / `[[a.b]]` (optionally trailing comment) into kind and name.
fn parse_header(trimmed: &str) -> Option<(String, String)> {
    let is_array = trimmed.starts_with("[[");
    let close = if is_array { "]]" } else { "]" };
    let open_len = if is_array { 2 } else { 1 };
    let rest = &trimmed[open_len..];
    let end = rest.find(close)?;
    let name = rest[..end].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let kind = if is_array { ARRAY_KIND } else { TABLE_KIND };
    Some((kind.to_string(), name))
}

/// Advance the cross-line lexer state over one line: enter/leave multi-line strings, count
/// array/inline-table brackets outside strings, and stop at an unquoted `#` comment.
fn scan_line(
    line: &str,
    in_basic_multiline: &mut bool,
    in_literal_multiline: &mut bool,
    depth: &mut i32,
) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if *in_basic_multiline {
            if bytes[i..].starts_with(b"\"\"\"") {
                *in_basic_multiline = false;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        if *in_literal_multiline {
            if bytes[i..].starts_with(b"'''") {
                *in_literal_multiline = false;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }

        let c = bytes[i];
        match c {
            b'#' => return, // comment to end of line
            b'"' => {
                if bytes[i..].starts_with(b"\"\"\"") {
                    *in_basic_multiline = true;
                    i += 3;
                } else {
                    i = skip_single_line_string(line, i + 1, b'"', true);
                }
            }
            b'\'' => {
                if bytes[i..].starts_with(b"'''") {
                    *in_literal_multiline = true;
                    i += 3;
                } else {
                    i = skip_single_line_string(line, i + 1, b'\'', false);
                }
            }
            b'[' | b'{' => {
                *depth += 1;
                i += 1;
            }
            b']' | b'}' => {
                *depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Return the index just past a single-line string opened at `start`, honoring `\` escapes for
/// basic (double-quoted) strings.
fn skip_single_line_string(line: &str, start: usize, quote: u8, escaped: bool) -> usize {
    let bytes = line.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if escaped && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn import_file(path: &str, content: &str) -> Vec<Operation> {
    let parsed = parse_toml(content);
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

fn render_file(graph: &SemanticGraph, file_symbol: &SymbolNode) -> String {
    let mut content = metadata_string(&file_symbol.metadata, "preamble").unwrap_or_default();
    for child in graph.children_of(file_symbol.id) {
        if child.kind != "file" {
            content.push_str(child.body.as_deref().unwrap_or(""));
        }
    }
    content
}

fn section_create(path: &str, file_id: Uuid, section: &Section) -> Operation {
    Operation::CreateSymbol {
        symbol_id: section_id(path, &section.kind, &section.name),
        parent_id: Some(file_id),
        kind: section.kind.clone(),
        name: section.name.clone(),
        body: Some(section.body.clone()),
        metadata: json!({}),
    }
}

fn file_metadata(path: &str, preamble: &str) -> serde_json::Value {
    json!({ "handler": "toml", "path": path, "preamble": preamble })
}

fn file_symbols(graph: &SemanticGraph) -> impl Iterator<Item = &SymbolNode> {
    graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
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
    stable_uuid(&format!("toml:file:{path}"))
}

fn section_id(path: &str, kind: &str, name: &str) -> Uuid {
    stable_uuid(&format!("toml:section:{path}:{kind}:{name}"))
}

fn diff_op_id(index: usize) -> Uuid {
    stable_uuid(&format!("toml:diff-op:{index}"))
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

    const DOC: &str = "# top comment\nname = \"demo\"\n\n[package]\nversion = \"1.0\"  # inline\n\n[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\nproptest = \"1\"\n";

    #[test]
    fn import_render_is_byte_identical_preserving_comments() {
        let graph = graph_from(&TomlHandler.import(&[rendered("Cargo.toml", DOC)]).unwrap());
        assert_eq!(TomlHandler.render(&graph)[0].content, DOC);
    }

    #[test]
    fn tables_become_sections_with_path_identity() {
        let operations = TomlHandler.import(&[rendered("Cargo.toml", DOC)]).unwrap();
        let names: BTreeSet<String> = operations
            .iter()
            .filter_map(|op| match op {
                Operation::CreateSymbol { kind, name, .. } if kind == TABLE_KIND => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            BTreeSet::from([
                "package".to_string(),
                "dependencies".to_string(),
                "dev-dependencies".to_string(),
            ])
        );
        // The leading top-level key and comment are preserved in the preamble.
        assert!(matches!(
            &operations[0],
            Operation::CreateSymbol { metadata, .. }
                if metadata["preamble"].as_str().unwrap().contains("name = \"demo\"")
        ));
    }

    #[test]
    fn editing_one_table_targets_only_that_symbol() {
        let graph = graph_from(&TomlHandler.import(&[rendered("Cargo.toml", DOC)]).unwrap());
        let scope: Vec<Uuid> = graph.root_symbols().iter().map(|s| s.id).collect();
        let edited = DOC.replace("serde = \"1\"", "serde = \"2\"");
        let operations = TomlHandler
            .recover_operations(&graph, &scope, &[rendered("Cargo.toml", &edited)])
            .unwrap();
        assert_eq!(
            operations.len(),
            1,
            "only the [dependencies] table changes: {operations:?}"
        );
        assert!(matches!(
            operations.as_slice(),
            [Operation::UpdateSymbol { body: Some(_), .. }]
        ));
    }

    #[test]
    fn array_of_tables_and_repeats_disambiguate() {
        let doc = "[[bin]]\nname = \"a\"\n\n[[bin]]\nname = \"b\"\n";
        let graph = graph_from(&TomlHandler.import(&[rendered("Cargo.toml", doc)]).unwrap());
        assert_eq!(TomlHandler.render(&graph)[0].content, doc);
        assert_eq!(
            graph
                .symbols
                .values()
                .filter(|s| s.kind == ARRAY_KIND)
                .count(),
            2
        );
    }

    #[test]
    fn bracket_at_line_start_inside_multiline_array_is_not_a_header() {
        // The `[3, 4]` element line begins with `[`, but it is inside an unclosed array value, so it
        // must not be treated as a table header.
        let doc = "[table]\nmatrix = [\n  [1, 2],\n  [3, 4],\n]\nkey = 1\n";
        let parsed = parse_toml(doc);
        assert_eq!(parsed.sections.len(), 1, "only [table] is a header");
        // Byte-stable regardless.
        let graph = graph_from(&TomlHandler.import(&[rendered("a.toml", doc)]).unwrap());
        assert_eq!(TomlHandler.render(&graph)[0].content, doc);
    }

    #[test]
    fn multiline_strings_with_non_ascii_do_not_panic_scanner() {
        let doc = "description = \"\"\"\nこれは\n\"\"\"\n\n[table]\nkey = 1\n";
        let parsed = parse_toml(doc);
        assert_eq!(parsed.sections.len(), 1);

        let graph = graph_from(&TomlHandler.import(&[rendered("a.toml", doc)]).unwrap());
        assert_eq!(TomlHandler.render(&graph)[0].content, doc);
    }
}
