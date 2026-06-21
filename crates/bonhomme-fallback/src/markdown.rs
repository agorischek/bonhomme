use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use bonhomme_core::{
    Handler, LanguagePlugin, Operation, RenderedFile, SemanticGraph, Slice, SymbolNode,
    ValidateFuture, metadata_string,
};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use serde_json::json;
use uuid::Uuid;

use crate::ids::stable_uuid;

#[cfg(test)]
mod tests;

/// Structured handler for Markdown. The document splits into sections by heading; each heading plus
/// the text up to the next heading is one symbol, so two branches editing different sections merge
/// and edits to the same section conflict. Identity is the heading path (`Usage > Examples`), which
/// disambiguates repeated headings under different parents. Bodies are the exact source spans and
/// render is span-splicing, so import → render is byte-identical — Markdown is naturally
/// text-preserving, unlike canonical JSON.
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownHandler;

const SECTION_KIND: &str = "section";

impl Handler for MarkdownHandler {
    fn name(&self) -> &str {
        "markdown"
    }

    fn claims(&self, file: &RenderedFile) -> bool {
        file.path.ends_with(".md") || file.path.ends_with(".markdown")
    }
}

impl LanguagePlugin for MarkdownHandler {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut files = file_symbols(graph)
            .map(|file_symbol| RenderedFile {
                path: file_path(file_symbol),
                content: render_markdown_file(graph, file_symbol),
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
                    content: render_markdown_file(graph, file_symbol),
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
            operations.extend(import_markdown_file(&file.path, &file.content));
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
        // Markdown has no compiler; like the blob tier it passes the merge gate opaquely. Safety
        // comes from section-granular conflicts, not validation.
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone, Debug)]
struct Section {
    /// Disambiguated heading path, e.g. `Usage > Examples` — the structural identity.
    name: String,
    title: String,
    level: usize,
    /// The exact source span from this heading up to the next heading.
    body: String,
}

/// Split Markdown into the leading preamble (text before the first heading) and its sections. Uses
/// the CommonMark parser so a `#` inside a fenced code block is never mistaken for a heading.
fn parse_markdown(content: &str) -> (String, Vec<Section>) {
    let mut anchors: Vec<(usize, usize, String)> = Vec::new();
    {
        let mut in_heading = false;
        let mut level = 1usize;
        let mut start = 0usize;
        let mut title = String::new();
        for (event, range) in Parser::new(content).into_offset_iter() {
            match event {
                Event::Start(Tag::Heading { level: l, .. }) => {
                    in_heading = true;
                    level = heading_level(l);
                    start = range.start;
                    title.clear();
                }
                Event::End(TagEnd::Heading(_)) => {
                    in_heading = false;
                    anchors.push((level, start, title.trim().to_string()));
                }
                Event::Text(text) | Event::Code(text) if in_heading => title.push_str(&text),
                _ => {}
            }
        }
    }

    if anchors.is_empty() {
        return (content.to_string(), Vec::new());
    }

    let preamble = content[..anchors[0].1].to_string();
    let mut sections = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut name_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (index, (level, start, title)) in anchors.iter().enumerate() {
        let end = anchors
            .get(index + 1)
            .map(|next| next.1)
            .unwrap_or(content.len());
        let body = content[*start..end].to_string();

        while let Some((ancestor_level, _)) = stack.last() {
            if *ancestor_level >= *level {
                stack.pop();
            } else {
                break;
            }
        }
        let mut path: Vec<String> = stack.iter().map(|(_, title)| title.clone()).collect();
        path.push(title.clone());
        stack.push((*level, title.clone()));

        let base_name = path.join(" > ");
        let count = name_counts.entry(base_name.clone()).or_insert(0);
        *count += 1;
        // Repeated identical heading paths get an occurrence suffix so the (parent, kind, name)
        // sibling key stays unique and the document still imports.
        let name = if *count == 1 {
            base_name
        } else {
            format!("{base_name} ({count})")
        };

        sections.push(Section {
            name,
            title: title.clone(),
            level: *level,
            body,
        });
    }

    (preamble, sections)
}

fn import_markdown_file(path: &str, content: &str) -> Vec<Operation> {
    let (preamble, sections) = parse_markdown(content);
    let file_id = md_file_id(path);
    let mut operations = vec![Operation::CreateSymbol {
        symbol_id: file_id,
        parent_id: None,
        kind: "file".to_string(),
        name: path.to_string(),
        body: None,
        metadata: file_metadata(path, &preamble),
    }];
    for section in sections {
        operations.push(section_create(path, file_id, &section));
    }
    operations
}

fn render_markdown_file(graph: &SemanticGraph, file_symbol: &SymbolNode) -> String {
    let mut content = metadata_string(&file_symbol.metadata, "preamble").unwrap_or_default();
    for child in graph.children_of(file_symbol.id) {
        if child.kind == SECTION_KIND {
            content.push_str(child.body.as_deref().unwrap_or(""));
        }
    }
    content
}

fn section_create(path: &str, file_id: Uuid, section: &Section) -> Operation {
    Operation::CreateSymbol {
        symbol_id: md_section_id(path, &section.name),
        parent_id: Some(file_id),
        kind: SECTION_KIND.to_string(),
        name: section.name.clone(),
        body: Some(section.body.clone()),
        metadata: json!({ "level": section.level, "title": section.title }),
    }
}

fn file_metadata(path: &str, preamble: &str) -> serde_json::Value {
    json!({ "handler": "markdown", "path": path, "preamble": preamble })
}

fn heading_level(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
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

fn md_file_id(path: &str) -> Uuid {
    stable_uuid(&format!("markdown:file:{path}"))
}

fn md_section_id(path: &str, name: &str) -> Uuid {
    stable_uuid(&format!("markdown:section:{path}:{name}"))
}

pub(crate) fn diff_op_id(index: usize) -> Uuid {
    stable_uuid(&format!("markdown:diff-op:{index}"))
}
