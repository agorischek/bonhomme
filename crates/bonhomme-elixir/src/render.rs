use bonhomme_core::{RenderedFile, SemanticGraph, Slice, SymbolNode, metadata_string};
use uuid::Uuid;

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
    let requested = root_symbols
        .iter()
        .filter_map(|symbol_id| graph.symbols.get(symbol_id))
        .collect::<Vec<_>>();

    let files = if requested.is_empty() {
        render_files(graph)
    } else {
        let mut files = Vec::new();
        for symbol in requested {
            let file_symbol = nearest_file_symbol(graph, symbol).unwrap_or(symbol);
            files.push(render_file(graph, file_symbol));
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        files.dedup_by(|left, right| left.path == right.path);
        files
    };

    Slice {
        id: Uuid::new_v4(),
        base_revision,
        root_symbols,
        files,
    }
}

fn render_file(graph: &SemanticGraph, file: &SymbolNode) -> RenderedFile {
    let path = metadata_string(&file.metadata, "path").unwrap_or_else(|| file.name.clone());
    let mut content = String::new();
    if let Some(preamble) = metadata_string(&file.metadata, "preamble")
        && !preamble.trim().is_empty()
    {
        content.push_str(preamble.trim());
        content.push_str("\n\n");
    }
    for child in graph.children_of(file.id) {
        render_symbol(graph, child, 0, &mut content);
    }
    RenderedFile {
        path,
        content: final_newline(content),
    }
}

fn render_symbol(graph: &SemanticGraph, symbol: &SymbolNode, indent: usize, out: &mut String) {
    match symbol.kind.as_str() {
        "module" => render_module(graph, symbol, indent, out),
        "function" | "macro" => {
            render_body_text(symbol.body.as_deref().unwrap_or(""), indent, out);
            out.push('\n');
        }
        _ => {}
    }
}

fn render_module(graph: &SemanticGraph, symbol: &SymbolNode, indent: usize, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("defmodule {}", symbol.name));
    write_indent(indent, out);
    out.push_str(signature.trim_end());
    out.push_str(" do\n");

    let mut wrote_body = false;
    if let Some(preamble) = metadata_string(&symbol.metadata, "bodyPreamble")
        && !preamble.trim().is_empty()
    {
        render_body_text(preamble.trim(), indent + 2, out);
        out.push('\n');
        wrote_body = true;
    }

    for child in graph.children_of(symbol.id) {
        match child.kind.as_str() {
            "module" | "function" | "macro" => {
                render_symbol(graph, child, indent + 2, out);
                wrote_body = true;
            }
            _ => {}
        }
    }

    if !wrote_body {
        write_indent(indent + 2, out);
        out.push_str("# empty\n");
    }
    write_indent(indent, out);
    out.push_str("end\n\n");
}

fn render_body_text(body: &str, indent: usize, out: &mut String) {
    for line in body.lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            write_indent(indent, out);
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
}

fn write_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn final_newline(mut content: String) -> String {
    while content.ends_with('\n') {
        content.pop();
    }
    content.push('\n');
    content
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
