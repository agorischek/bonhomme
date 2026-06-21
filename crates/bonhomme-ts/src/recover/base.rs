use anyhow::{Context, Result};
use bonhomme_core::{SemanticGraph, SymbolNode, metadata_bool, metadata_string};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(super) struct BaseFile {
    pub(super) id: Uuid,
    pub(super) path: String,
    pub(super) preamble: String,
    pub(super) classes: Vec<BaseClass>,
    pub(super) functions: Vec<BaseFunction>,
}

#[derive(Clone, Debug)]
pub(super) struct BaseClass {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) declaration: String,
    pub(super) doc: Option<String>,
    pub(super) methods: Vec<BaseMethod>,
    pub(super) properties: Vec<BaseProperty>,
}

#[derive(Clone, Debug)]
pub(super) struct BaseProperty {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) declaration: String,
    pub(super) doc: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct BaseFunction {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) signature: String,
    pub(super) body: String,
    pub(super) doc: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct BaseMethod {
    pub(super) id: Uuid,
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) signature: String,
    pub(super) body: String,
    pub(super) method_kind: String,
    pub(super) is_static: bool,
    pub(super) doc: Option<String>,
}

pub(super) fn base_files_by_path(
    base: &SemanticGraph,
    scope: &[Uuid],
) -> Result<BTreeMap<String, BaseFile>> {
    let mut file_ids = BTreeSet::new();
    if scope.is_empty() {
        file_ids.extend(
            base.root_symbols()
                .into_iter()
                .filter(|symbol| symbol.kind == "file")
                .map(|symbol| symbol.id),
        );
    } else {
        for symbol_id in scope {
            let symbol = base
                .symbols
                .get(symbol_id)
                .with_context(|| format!("scope symbol {symbol_id} does not exist"))?;
            let file = nearest_file_symbol(base, symbol)
                .with_context(|| format!("scope symbol {symbol_id} has no containing file"))?;
            file_ids.insert(file.id);
        }
    }

    let mut files = BTreeMap::new();
    for file_id in file_ids {
        let file = base
            .symbols
            .get(&file_id)
            .with_context(|| format!("file symbol {file_id} does not exist"))?;
        let base_file = base_file(base, file);
        files.insert(base_file.path.clone(), base_file);
    }
    Ok(files)
}

fn base_file(base: &SemanticGraph, file: &SymbolNode) -> BaseFile {
    let path = metadata_string(&file.metadata, "path").unwrap_or_else(|| file.name.clone());
    let preamble = metadata_string(&file.metadata, "preamble").unwrap_or_default();
    let mut classes = Vec::new();
    let mut functions = Vec::new();
    for child in base.children_of(file.id) {
        match child.kind.as_str() {
            "class" => classes.push(base_class(base, child)),
            "function" => functions.push(BaseFunction {
                id: child.id,
                name: child.name.clone(),
                signature: function_signature(child),
                body: child.body.clone().unwrap_or_default(),
                doc: metadata_string(&child.metadata, "doc"),
            }),
            _ => {}
        }
    }
    BaseFile {
        id: file.id,
        path,
        preamble,
        classes,
        functions,
    }
}

fn base_class(base: &SemanticGraph, class: &SymbolNode) -> BaseClass {
    let children = base.children_of(class.id);
    let methods = children
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind.as_str(),
                "method"
                    | "static-method"
                    | "getter"
                    | "static-getter"
                    | "setter"
                    | "static-setter"
            )
        })
        .map(|method| BaseMethod {
            id: method.id,
            kind: method.kind.clone(),
            name: method.name.clone(),
            signature: metadata_string(&method.metadata, "signature")
                .unwrap_or_else(|| format!("{}(): void", method.name)),
            body: method.body.clone().unwrap_or_default(),
            method_kind: metadata_string(&method.metadata, "methodKind")
                .unwrap_or_else(|| "Method".to_string()),
            is_static: metadata_bool(&method.metadata, "static"),
            doc: metadata_string(&method.metadata, "doc"),
        })
        .collect();
    let properties = children
        .iter()
        .filter(|symbol| symbol.kind == "property")
        .map(|property| BaseProperty {
            id: property.id,
            name: property.name.clone(),
            declaration: metadata_string(&property.metadata, "declaration")
                .unwrap_or_else(|| format!("{}: unknown;", property.name)),
            doc: metadata_string(&property.metadata, "doc"),
        })
        .collect();

    BaseClass {
        id: class.id,
        name: class.name.clone(),
        declaration: metadata_string(&class.metadata, "declaration")
            .unwrap_or_else(|| format!("class {}", class.name)),
        doc: metadata_string(&class.metadata, "doc"),
        methods,
        properties,
    }
}

fn nearest_file_symbol<'a>(
    graph: &'a SemanticGraph,
    symbol: &'a SymbolNode,
) -> Option<&'a SymbolNode> {
    if symbol.kind == "file" {
        return Some(symbol);
    }

    let mut current = symbol;
    while let Some(parent_id) = current.parent_id {
        current = graph.symbols.get(&parent_id)?;
        if current.kind == "file" {
            return Some(current);
        }
    }
    None
}

fn function_signature(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "declaration").unwrap_or_else(|| {
        metadata_string(&symbol.metadata, "signature")
            .unwrap_or_else(|| format!("function {}(): void", symbol.name))
    })
}
