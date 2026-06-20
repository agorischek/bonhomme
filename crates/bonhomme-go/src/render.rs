use crate::toolchain::format_go_source;
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
    let package_name =
        metadata_string(&file.metadata, "package").unwrap_or_else(|| "main".to_string());
    let mut content = String::new();
    content.push_str(&format!("package {package_name}\n\n"));

    if let Some(imports) = metadata_string(&file.metadata, "imports")
        && !imports.trim().is_empty()
    {
        content.push_str(imports.trim());
        content.push_str("\n\n");
    }

    for child in graph.children_of(file.id) {
        render_top_level_symbol(graph, child, &mut content);
    }

    let content = format_go_source(&content).unwrap_or(content);
    RenderedFile { path, content }
}

fn render_top_level_symbol(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    match symbol.kind.as_str() {
        "struct" => render_struct(graph, symbol, out),
        "interface" => render_interface(graph, symbol, out),
        "function" => render_function(symbol, out),
        "const" | "var" | "type" => render_declaration(symbol, out),
        _ => {}
    }
}

fn render_struct(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let declaration = metadata_string(&symbol.metadata, "declaration")
        .unwrap_or_else(|| format!("type {} struct", symbol.name));
    out.push_str(&declaration);
    out.push_str(" {\n");
    for child in graph.children_of(symbol.id) {
        if child.kind == "field"
            && let Some(declaration) = metadata_string(&child.metadata, "declaration")
        {
            out.push('\t');
            out.push_str(declaration.trim());
            out.push('\n');
        }
    }
    out.push_str("}\n\n");

    for child in graph.children_of(symbol.id) {
        if child.kind == "method" && child.body.is_some() {
            render_function(child, out);
        }
    }
}

fn render_interface(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let declaration = metadata_string(&symbol.metadata, "declaration")
        .unwrap_or_else(|| format!("type {} interface", symbol.name));
    out.push_str(&declaration);
    out.push_str(" {\n");
    for child in graph.children_of(symbol.id) {
        if child.kind == "method" && child.body.is_none() {
            let signature = metadata_string(&child.metadata, "signature")
                .unwrap_or_else(|| format!("{}()", child.name));
            out.push('\t');
            out.push_str(signature.trim());
            out.push('\n');
        }
    }
    out.push_str("}\n\n");
}

fn render_function(symbol: &SymbolNode, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("func {}()", symbol.name));
    out.push_str(signature.trim());
    out.push_str(" {\n");
    if let Some(body) = &symbol.body {
        for line in body.lines() {
            out.push('\t');
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.push_str("}\n\n");
}

fn render_declaration(symbol: &SymbolNode, out: &mut String) {
    if let Some(declaration) = metadata_string(&symbol.metadata, "declaration") {
        out.push_str(declaration.trim());
        out.push_str("\n\n");
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
