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

    let namespace_style = metadata_string(&file.metadata, "namespaceStyle");
    if namespace_style.as_deref() == Some("block") {
        let namespace_header = metadata_string(&file.metadata, "namespaceHeader")
            .or_else(|| {
                metadata_string(&file.metadata, "namespace").map(|name| format!("namespace {name}"))
            })
            .unwrap_or_else(|| "namespace".to_string());
        render_text_block(namespace_header.trim(), 0, &mut content);
        content.push_str("{\n");
        if let Some(body_preamble) = metadata_string(&file.metadata, "namespaceBodyPreamble")
            && !body_preamble.trim().is_empty()
        {
            render_body_text(body_preamble.trim(), 4, &mut content);
        }
        render_file_declarations(graph, file.id, 4, &mut content);
        content.push_str("}\n");
    } else {
        render_file_declarations(graph, file.id, 0, &mut content);
    }

    RenderedFile {
        path,
        content: final_newline(content),
    }
}

fn render_file_declarations(
    graph: &SemanticGraph,
    parent_id: Uuid,
    indent: usize,
    out: &mut String,
) {
    for child in graph.children_of(parent_id) {
        if is_type_kind(&child.kind) {
            render_type(graph, child, indent, out);
        } else if child.kind == "function" {
            render_callable(child, indent, out);
            out.push('\n');
        }
    }
}

fn render_type(graph: &SemanticGraph, symbol: &SymbolNode, indent: usize, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("{} {}", symbol.kind, symbol.name));
    render_signature(signature.trim(), indent, None, out);
    write_indent(indent, out);
    out.push_str("{\n");

    if let Some(preamble) = metadata_string(&symbol.metadata, "bodyPreamble")
        && !preamble.trim().is_empty()
    {
        render_body_text(preamble.trim(), indent + 4, out);
    }
    for child in graph.children_of(symbol.id) {
        match child.kind.as_str() {
            "method" => render_callable(child, indent + 4, out),
            "property" | "constant" | "enum-case" | "trait-use" => {
                render_declaration(child, indent + 4, out)
            }
            _ => {}
        }
    }

    write_indent(indent, out);
    out.push_str("}\n\n");
}

fn render_callable(symbol: &SymbolNode, indent: usize, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("function {}()", symbol.name));
    let body_kind = metadata_string(&symbol.metadata, "declaration").unwrap_or_default();
    match (body_kind.as_str(), symbol.body.as_deref()) {
        ("none", _) | (_, None) => render_signature(signature.trim(), indent, Some(";"), out),
        (_, Some(body)) => {
            render_signature(signature.trim(), indent, None, out);
            write_indent(indent, out);
            out.push_str("{\n");
            render_body_text(body, indent + 4, out);
            write_indent(indent, out);
            out.push_str("}\n");
        }
    }
}

fn render_declaration(symbol: &SymbolNode, indent: usize, out: &mut String) {
    if let Some(declaration) = metadata_string(&symbol.metadata, "declaration") {
        render_text_block(declaration.trim(), indent, out);
    }
}

fn render_signature(text: &str, indent: usize, suffix: Option<&str>, out: &mut String) {
    let lines = text.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        write_indent(indent, out);
        out.push_str(line.trim_end());
        if index + 1 == lines.len()
            && let Some(suffix) = suffix
        {
            out.push_str(suffix);
        }
        out.push('\n');
    }
}

fn render_text_block(text: &str, indent: usize, out: &mut String) {
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            write_indent(indent, out);
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
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

fn is_type_kind(kind: &str) -> bool {
    matches!(kind, "class" | "interface" | "trait" | "enum")
}
