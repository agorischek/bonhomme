use std::collections::BTreeSet;

use bonhomme_core::{RenderedFile, SemanticGraph, Slice, SymbolNode, metadata_string};
use uuid::Uuid;

use crate::model::{FRONTMATTER_KIND, SECTION_KIND};

pub fn render_files(graph: &SemanticGraph) -> Vec<RenderedFile> {
    let mut files = graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
        .map(|file| render_file(graph, file))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

pub fn render_slice(
    graph: &SemanticGraph,
    base_revision: String,
    root_symbols: Vec<Uuid>,
) -> Slice {
    let file_ids: Vec<Uuid> = if root_symbols.is_empty() {
        graph
            .root_symbols()
            .into_iter()
            .filter(|symbol| symbol.kind == "file")
            .map(|symbol| symbol.id)
            .collect()
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
            && let Some(file) = graph.symbols.get(&id)
        {
            files.push(render_file(graph, file));
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

fn render_file(graph: &SemanticGraph, file: &SymbolNode) -> RenderedFile {
    let path = file_path(file);
    let mut content = String::new();

    for child in graph.children_of(file.id) {
        if child.kind == FRONTMATTER_KIND {
            content.push_str(child.body.as_deref().unwrap_or(""));
        }
    }

    content.push_str(&metadata_string(&file.metadata, "preamble").unwrap_or_default());
    for child in graph.children_of(file.id) {
        if child.kind == SECTION_KIND {
            render_section(graph, child, &mut content);
        }
    }

    RenderedFile { path, content }
}

fn render_section(graph: &SemanticGraph, section: &SymbolNode, content: &mut String) {
    content.push_str(section.body.as_deref().unwrap_or(""));
    for child in graph.children_of(section.id) {
        if child.kind == SECTION_KIND {
            render_section(graph, child, content);
        }
    }
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

fn file_path(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone())
}
