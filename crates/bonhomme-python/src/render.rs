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
        render_top_level_symbol(graph, child, &mut content);
    }
    RenderedFile {
        path,
        content: final_newline(content),
    }
}

fn render_top_level_symbol(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    match symbol.kind.as_str() {
        "class" => render_class(graph, symbol, out),
        "function" => {
            render_function(symbol, 0, out);
            out.push('\n');
        }
        "value" => render_declaration(symbol, 0, out),
        _ => {}
    }
}

fn render_class(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("class {}", symbol.name));
    render_header(&signature, 0, out);

    let mut wrote_body = false;
    if let Some(preamble) = metadata_string(&symbol.metadata, "bodyPreamble")
        && !preamble.trim().is_empty()
    {
        render_body_text(preamble.trim(), 4, out);
        wrote_body = true;
    }

    for child in graph.children_of(symbol.id) {
        match child.kind.as_str() {
            "attribute" => {
                render_declaration(child, 4, out);
                wrote_body = true;
            }
            "method" => {
                render_function(child, 4, out);
                wrote_body = true;
            }
            _ => {}
        }
    }

    if !wrote_body {
        write_indent(4, out);
        out.push_str("pass\n");
    }
    out.push('\n');
}

fn render_function(symbol: &SymbolNode, indent: usize, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("def {}()", symbol.name));
    render_header(&signature, indent, out);
    match symbol.body.as_deref() {
        Some(body) if !body.trim().is_empty() => render_body_text(body, indent + 4, out),
        _ => {
            write_indent(indent + 4, out);
            out.push_str("pass\n");
        }
    }
}

fn render_declaration(symbol: &SymbolNode, indent: usize, out: &mut String) {
    if let Some(declaration) = metadata_string(&symbol.metadata, "declaration") {
        for line in declaration.lines() {
            write_indent(indent, out);
            out.push_str(line.trim_end());
            out.push('\n');
        }
        if indent == 0 {
            out.push('\n');
        }
    }
}

fn render_header(signature: &str, indent: usize, out: &mut String) {
    let lines = signature.trim_end().lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        write_indent(indent, out);
        out.push_str(line.trim_end());
        if index + 1 == lines.len() {
            out.push(':');
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
