use bonhomme_core::{RenderedFile, SemanticGraph, Slice, SymbolNode, metadata_string};
use std::collections::BTreeMap;
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
    render_uses(file, &mut content);
    for child in graph.children_of(file.id) {
        render_top_level_symbol(graph, child, &mut content);
    }
    let content = format_rust_source(&content);
    RenderedFile { path, content }
}

fn render_uses(file: &SymbolNode, out: &mut String) {
    let Some(uses) = file.metadata.get("uses").and_then(|value| value.as_array()) else {
        return;
    };
    for item in uses.iter().filter_map(|value| value.as_str()) {
        out.push_str(item.trim());
        out.push_str("\n\n");
    }
}

fn render_top_level_symbol(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    match symbol.kind.as_str() {
        "struct" => render_struct(graph, symbol, out),
        "enum" => render_enum(graph, symbol, out),
        "trait" => render_trait(graph, symbol, out),
        "function" => render_function(symbol, out),
        "impl" => render_impl(graph, symbol, out),
        "const" | "static" | "type" | "raw" => render_declaration(symbol, out),
        _ => {}
    }
}

fn render_struct(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let declaration = metadata_string(&symbol.metadata, "declaration")
        .unwrap_or_else(|| format!("struct {}", symbol.name));
    let fields = graph
        .children_of(symbol.id)
        .into_iter()
        .filter(|child| child.kind == "field")
        .collect::<Vec<_>>();

    if fields.is_empty() {
        out.push_str(&declaration);
        out.push_str(";\n\n");
    } else if fields.iter().all(|field| field.name.contains('.')) {
        out.push_str(&declaration);
        out.push('(');
        render_members(&fields, out);
        out.push_str(");\n\n");
    } else {
        out.push_str(&declaration);
        out.push_str(" {\n");
        render_members(&fields, out);
        out.push_str("}\n\n");
    }

    render_associated_impls(graph, symbol, out);
}

fn render_enum(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let declaration = metadata_string(&symbol.metadata, "declaration")
        .unwrap_or_else(|| format!("enum {}", symbol.name));
    out.push_str(&declaration);
    out.push_str(" {\n");
    let variants = graph
        .children_of(symbol.id)
        .into_iter()
        .filter(|child| child.kind == "variant")
        .collect::<Vec<_>>();
    render_members(&variants, out);
    out.push_str("}\n\n");
    render_associated_impls(graph, symbol, out);
}

fn render_trait(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let declaration = metadata_string(&symbol.metadata, "declaration")
        .unwrap_or_else(|| format!("trait {}", symbol.name));
    out.push_str(&declaration);
    out.push_str(" {\n");
    for child in graph.children_of(symbol.id) {
        if child.kind == "method" {
            render_trait_method(child, out);
        }
    }
    out.push_str("}\n\n");
}

fn render_associated_impls(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let mut groups: BTreeMap<String, Vec<&SymbolNode>> = BTreeMap::new();
    for child in graph.children_of(symbol.id) {
        if child.kind == "method" && child.body.is_some() {
            let header = metadata_string(&child.metadata, "implHeader")
                .unwrap_or_else(|| format!("impl {}", symbol.name));
            groups.entry(header).or_default().push(child);
        }
    }
    for (header, methods) in groups {
        out.push_str(header.trim());
        out.push_str(" {\n");
        for method in methods {
            render_method(method, out);
        }
        out.push_str("}\n\n");
    }
}

fn render_impl(graph: &SemanticGraph, symbol: &SymbolNode, out: &mut String) {
    let declaration = metadata_string(&symbol.metadata, "declaration")
        .unwrap_or_else(|| format!("impl {}", symbol.name));
    out.push_str(declaration.trim());
    out.push_str(" {\n");
    for child in graph.children_of(symbol.id) {
        if child.kind == "method" {
            render_method(child, out);
        }
    }
    out.push_str("}\n\n");
}

fn render_members(members: &[&SymbolNode], out: &mut String) {
    for member in members {
        if let Some(declaration) = metadata_string(&member.metadata, "declaration") {
            out.push_str(declaration.trim());
            out.push_str(",\n");
        }
    }
}

fn render_trait_method(symbol: &SymbolNode, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("fn {}()", symbol.name));
    out.push_str(signature.trim());
    if let Some(body) = &symbol.body {
        out.push_str(" {\n");
        out.push_str(body.trim());
        out.push_str("\n}\n");
    } else {
        out.push_str(";\n");
    }
}

fn render_method(symbol: &SymbolNode, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("fn {}()", symbol.name));
    out.push_str(signature.trim());
    out.push_str(" {\n");
    if let Some(body) = &symbol.body {
        out.push_str(body.trim());
        out.push('\n');
    }
    out.push_str("}\n");
}

fn render_function(symbol: &SymbolNode, out: &mut String) {
    let signature = metadata_string(&symbol.metadata, "signature")
        .unwrap_or_else(|| format!("fn {}()", symbol.name));
    out.push_str(signature.trim());
    out.push_str(" {\n");
    if let Some(body) = &symbol.body {
        out.push_str(body.trim());
        out.push('\n');
    }
    out.push_str("}\n\n");
}

fn render_declaration(symbol: &SymbolNode, out: &mut String) {
    if let Some(declaration) = metadata_string(&symbol.metadata, "declaration") {
        out.push_str(declaration.trim());
        out.push_str("\n\n");
    }
}

fn format_rust_source(content: &str) -> String {
    syn::parse_file(content)
        .map(|file| prettyplease::unparse(&file))
        .unwrap_or_else(|_| content.to_string())
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
