use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, SymbolNode,
    ValidateFuture, metadata_string,
};
use serde_json::json;
use tree_sitter::{Language, Node, Parser};
use uuid::Uuid;

use crate::ids::stable_uuid;

/// Tree-sitter structural-lite tier. One dependency (tree-sitter plus a few grammars) buys
/// *top-level-symbol* granularity for languages that have no full plugin: a CST is parsed, each
/// top-level named declaration becomes a symbol whose body is its exact source span, and the text
/// between declarations becomes preamble/trailer. Render splices the spans back by stored ranges, so
/// import → render is byte-identical.
///
/// There is no validator (we cannot compile these languages here), but — crucially — it still
/// conflicts on same-symbol edits and never line-merges within a symbol, so it stays on the safe
/// side of "surface conflicts, do not guess". It is the natural on-ramp to a full plugin: a language
/// starts here and graduates to a hand-tuned plugin with a validator when it earns one.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeSitterHandler;

struct Grammar {
    /// Stored on each symbol's metadata and used to re-select the grammar during recover.
    name: &'static str,
    extensions: &'static [&'static str],
    language: fn() -> Language,
}

fn python_language() -> Language {
    tree_sitter_python::LANGUAGE.into()
}

fn rust_language() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}

const GRAMMARS: &[Grammar] = &[
    Grammar {
        name: "python",
        extensions: &["py", "pyi"],
        language: python_language,
    },
    Grammar {
        name: "rust",
        extensions: &["rs"],
        language: rust_language,
    },
];

impl Handler for TreeSitterHandler {
    fn name(&self) -> &str {
        "treesitter"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        grammar_for_path(&file.path).is_some()
    }
}

impl LanguagePlugin for TreeSitterHandler {
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
            operations.extend(import_file(&file.path, &file.content)?);
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
        // No compiler is invoked: this tier trades validation for breadth. Safety comes from
        // same-symbol conflicts, exactly like the blob and markdown tiers.
        Box::pin(async { Ok(()) })
    }
}

/// A parsed top-level declaration: its kind, a (sibling-unique) name, and the exact source span
/// from the end of the previous declaration through the end of this one (so concatenation in order
/// reconstructs the file).
struct Decl {
    kind: String,
    name: String,
    body: String,
}

struct Parsed {
    preamble: String,
    trailer: String,
    decls: Vec<Decl>,
}

fn parse_source(grammar: &Grammar, source: &str) -> Result<Parsed> {
    let mut parser = Parser::new();
    parser
        .set_language(&(grammar.language)())
        .map_err(|error| anyhow::anyhow!("failed to load {} grammar: {error}", grammar.name))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse {} source", grammar.name))?;
    let root = tree.root_node();
    if root.has_error() {
        bail!("{} source has syntax errors", grammar.name);
    }

    let bytes = source.as_bytes();
    let mut raw: Vec<(usize, usize, String, String)> = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if let Some((kind, name)) = classify(grammar.name, child, bytes) {
            raw.push((child.start_byte(), child.end_byte(), kind, name));
        }
    }

    if raw.is_empty() {
        return Ok(Parsed {
            preamble: source.to_string(),
            trailer: String::new(),
            decls: Vec::new(),
        });
    }

    let preamble = source[..raw[0].0].to_string();
    let mut decls = Vec::new();
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut prev_end = raw[0].0;
    for (_, end, kind, name) in &raw {
        let body = source[prev_end..*end].to_string();
        prev_end = *end;

        // Disambiguate repeated (kind, name) siblings (e.g. several `impl Foo` blocks) so the graph's
        // sibling-uniqueness invariant holds and the file still imports.
        let key = format!("{kind}\u{0}{name}");
        let count = name_counts.entry(key).or_insert(0);
        *count += 1;
        let unique_name = if *count == 1 {
            name.clone()
        } else {
            format!("{name} ({count})")
        };

        decls.push(Decl {
            kind: kind.clone(),
            name: unique_name,
            body,
        });
    }
    let trailer = source[prev_end..].to_string();

    Ok(Parsed {
        preamble,
        trailer,
        decls,
    })
}

/// Map a top-level CST node to a `(kind, name)` for the grammar, or `None` if it is not a named
/// declaration we model (it then stays as inter-symbol text).
fn classify(language: &str, node: Node, source: &[u8]) -> Option<(String, String)> {
    match language {
        "python" => classify_python(node, source),
        "rust" => classify_rust(node, source),
        _ => None,
    }
}

fn classify_python(node: Node, source: &[u8]) -> Option<(String, String)> {
    match node.kind() {
        "function_definition" => Some(("function".to_string(), field_name(node, source)?)),
        "class_definition" => Some(("class".to_string(), field_name(node, source)?)),
        "decorated_definition" => {
            let inner = node.child_by_field_name("definition")?;
            let kind = match inner.kind() {
                "function_definition" => "function",
                "class_definition" => "class",
                _ => return None,
            };
            Some((kind.to_string(), field_name(inner, source)?))
        }
        _ => None,
    }
}

fn classify_rust(node: Node, source: &[u8]) -> Option<(String, String)> {
    let kind = match node.kind() {
        "function_item" => "function",
        "struct_item" => "struct",
        "enum_item" => "enum",
        "union_item" => "union",
        "trait_item" => "trait",
        "mod_item" => "mod",
        "const_item" => "const",
        "static_item" => "static",
        "type_item" => "type",
        "macro_definition" => "macro",
        "impl_item" => "impl",
        _ => return None,
    };
    let name = if node.kind() == "impl_item" {
        let type_text = node_text(node.child_by_field_name("type")?, source)?;
        match node.child_by_field_name("trait") {
            Some(trait_node) => format!("{} for {type_text}", node_text(trait_node, source)?),
            None => type_text,
        }
    } else {
        field_name(node, source)?
    };
    Some((kind.to_string(), name))
}

fn field_name(node: Node, source: &[u8]) -> Option<String> {
    node_text(node.child_by_field_name("name")?, source)
}

fn node_text(node: Node, source: &[u8]) -> Option<String> {
    node.utf8_text(source).ok().map(ToString::to_string)
}

fn import_file(path: &str, content: &str) -> Result<Vec<Operation>> {
    let grammar = grammar_for_path(path)
        .ok_or_else(|| anyhow::anyhow!("no tree-sitter grammar claims {path}"))?;
    let parsed = parse_source(grammar, content)?;
    let file_id = file_id(path);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: file_id,
        parent_id: None,
        kind: "file".to_string(),
        name: path.to_string(),
        body: None,
        metadata: file_metadata(path, grammar.name, &parsed.preamble, &parsed.trailer),
    }];
    for decl in &parsed.decls {
        operations.push(decl_create(path, file_id, decl));
    }
    Ok(operations)
}

fn render_file(graph: &SemanticGraph, file_symbol: &SymbolNode) -> String {
    let mut content = metadata_string(&file_symbol.metadata, "preamble").unwrap_or_default();
    for child in graph.children_of(file_symbol.id) {
        if child.kind != "file" {
            content.push_str(child.body.as_deref().unwrap_or(""));
        }
    }
    content.push_str(&metadata_string(&file_symbol.metadata, "trailer").unwrap_or_default());
    content
}

fn decl_create(path: &str, file_id: Uuid, decl: &Decl) -> Operation {
    Operation::CreateSymbol {
        symbol_id: decl_id(path, &decl.kind, &decl.name),
        parent_id: Some(file_id),
        kind: decl.kind.clone(),
        name: decl.name.clone(),
        body: Some(decl.body.clone()),
        metadata: json!({}),
    }
}

fn file_metadata(path: &str, language: &str, preamble: &str, trailer: &str) -> serde_json::Value {
    json!({
        "handler": "treesitter",
        "path": path,
        "language": language,
        "preamble": preamble,
        "trailer": trailer,
    })
}

fn grammar_for_path(path: &str) -> Option<&'static Grammar> {
    let extension = path.rsplit('.').next()?;
    if !path.contains('.') {
        return None;
    }
    GRAMMARS
        .iter()
        .find(|grammar| grammar.extensions.contains(&extension))
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
    stable_uuid(&format!("treesitter:file:{path}"))
}

fn decl_id(path: &str, kind: &str, name: &str) -> Uuid {
    stable_uuid(&format!("treesitter:decl:{path}:{kind}:{name}"))
}

fn diff_op_id(index: usize) -> Uuid {
    stable_uuid(&format!("treesitter:diff-op:{index}"))
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

    const PY: &str = "import os\n\n\ndef greet(name):\n    return f\"hi {name}\"\n\n\nclass Service:\n    def run(self):\n        return 1\n";

    const RS: &str = "use std::fmt;\n\nfn helper() -> u32 {\n    1\n}\n\nstruct Point {\n    x: i32,\n}\n\nimpl Point {\n    fn new() -> Self {\n        Point { x: 0 }\n    }\n}\n";

    #[test]
    fn python_import_render_is_byte_identical() {
        let graph = graph_from(&TreeSitterHandler.import(&[rendered("a.py", PY)]).unwrap());
        assert_eq!(TreeSitterHandler.render(&graph)[0].content, PY);
    }

    #[test]
    fn python_top_level_decls_become_symbols() {
        let operations = TreeSitterHandler.import(&[rendered("a.py", PY)]).unwrap();
        let kinds: BTreeSet<(String, String)> = operations
            .iter()
            .filter_map(|op| match op {
                Operation::CreateSymbol { kind, name, .. } if kind != "file" => {
                    Some((kind.clone(), name.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            BTreeSet::from([
                ("function".to_string(), "greet".to_string()),
                ("class".to_string(), "Service".to_string()),
            ])
        );
    }

    #[test]
    fn rust_import_render_is_byte_identical() {
        let graph = graph_from(&TreeSitterHandler.import(&[rendered("a.rs", RS)]).unwrap());
        assert_eq!(TreeSitterHandler.render(&graph)[0].content, RS);
    }

    #[test]
    fn editing_one_function_targets_only_that_symbol() {
        let graph = graph_from(&TreeSitterHandler.import(&[rendered("a.rs", RS)]).unwrap());
        let scope: Vec<Uuid> = graph.root_symbols().iter().map(|s| s.id).collect();
        let edited = RS.replace("    1\n", "    42\n");
        let operations = TreeSitterHandler
            .recover_operations(&graph, &scope, &[rendered("a.rs", &edited)])
            .unwrap();
        // Exactly one symbol body updated (the `helper` function); nothing else touched.
        assert_eq!(operations.len(), 1, "got {operations:?}");
        assert!(matches!(
            operations.as_slice(),
            [Operation::UpdateSymbol { body: Some(_), .. }]
        ));
    }

    #[test]
    fn syntactically_broken_source_errors_so_the_router_can_degrade() {
        // A parse error returns Err, which the router turns into a blob fallback.
        let broken = "fn oops( {\n";
        assert!(
            TreeSitterHandler
                .import(&[rendered("a.rs", broken)])
                .is_err()
        );
    }

    #[test]
    fn claims_only_grammar_extensions() {
        assert!(TreeSitterHandler.claims(&rendered("x.py", "")));
        assert!(TreeSitterHandler.claims(&rendered("x.rs", "")));
        assert!(!TreeSitterHandler.claims(&rendered("x.ts", "")));
        assert!(!TreeSitterHandler.claims(&rendered("README", "")));
    }
}
