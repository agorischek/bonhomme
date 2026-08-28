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

    render_type_symbols(graph, file.id, 0, &mut content);

    RenderedFile {
        path,
        content: final_newline(content),
    }
}

fn render_type_symbols(graph: &SemanticGraph, parent_id: Uuid, indent: usize, out: &mut String) {
    for child in graph.children_of(parent_id) {
        if is_type_kind(&child.kind) {
            render_type(graph, child, indent, out);
        }
    }
}

fn render_type(graph: &SemanticGraph, symbol: &SymbolNode, indent: usize, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("{} {}", symbol.kind, symbol.name));
    render_text_block(signature.trim(), indent, out);
    write_indent(indent, out);
    out.push_str("{\n");

    if let Some(preamble) = metadata_string(&symbol.metadata, "bodyPreamble")
        && !preamble.trim().is_empty()
    {
        render_body_text(preamble.trim(), indent + 4, out);
    }
    for child in graph.children_of(symbol.id) {
        if is_type_kind(&child.kind) {
            render_type(graph, child, indent + 4, out);
            continue;
        }
        match child.kind.as_str() {
            "field" | "annotation-element" | "enum-constant" | "initializer"
            | "static-initializer" => render_declaration(child, indent + 4, out),
            "method" | "constructor" => render_callable(child, indent + 4, out),
            _ => {}
        }
    }

    write_indent(indent, out);
    out.push_str("}\n\n");
}

fn render_callable(symbol: &SymbolNode, indent: usize, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("void {}()", symbol.name));
    let body_kind = metadata_string(&symbol.metadata, "declaration").unwrap_or_default();
    match (body_kind.as_str(), symbol.body.as_deref()) {
        ("none", _) | (_, None) => {
            render_signature(signature.trim(), indent, Some(";"), out);
        }
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
    matches!(
        kind,
        "class" | "record" | "interface" | "annotation" | "enum"
    )
}
