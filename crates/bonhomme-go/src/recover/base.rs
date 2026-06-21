use crate::import::scope_from_path;
use bonhomme_core::{SemanticGraph, SymbolNode, metadata_string};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(super) struct BaseSymbol {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) signature: String,
    pub(super) declaration: String,
    pub(super) body: String,
    pub(super) doc: Option<String>,
}

pub(super) fn base_file_paths(base: &SemanticGraph) -> BTreeMap<String, Uuid> {
    base.root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
        .map(|symbol| {
            (
                metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone()),
                symbol.id,
            )
        })
        .collect()
}

pub(super) fn children_of_kind(
    base: &SemanticGraph,
    parent_id: Uuid,
    kind: &str,
) -> Vec<BaseSymbol> {
    base.children_of(parent_id)
        .into_iter()
        .filter(|symbol| symbol.kind == kind)
        .map(base_symbol)
        .collect()
}

pub(super) fn methods_for_receiver(
    base: &SemanticGraph,
    type_symbol_id: Uuid,
    scope: &str,
    receiver: &str,
) -> Vec<BaseSymbol> {
    let mut methods = children_of_kind(base, type_symbol_id, "method");
    methods.extend(
        base.symbols
            .values()
            .filter(|symbol| {
                symbol.kind == "method"
                    && metadata_string(&symbol.metadata, "receiver").as_deref() == Some(receiver)
                    && symbol_scope(base, symbol).as_deref() == Some(scope)
                    && nearest_file_symbol(base, symbol)
                        .is_some_and(|file| symbol.parent_id == Some(file.id))
            })
            .map(base_symbol),
    );
    methods.sort_by(|left, right| left.id.cmp(&right.id));
    methods.dedup_by_key(|method| method.id);
    methods
}

pub(super) fn base_type_by_name(
    base: &SemanticGraph,
    scope: &str,
    name: &str,
) -> Option<BaseSymbol> {
    base.symbols
        .values()
        .find(|symbol| {
            matches!(symbol.kind.as_str(), "struct" | "interface" | "type")
                && symbol.name == name
                && symbol_scope(base, symbol).as_deref() == Some(scope)
        })
        .map(base_symbol)
}

pub(super) fn symbol_scope(base: &SemanticGraph, symbol: &SymbolNode) -> Option<String> {
    let file = nearest_file_symbol(base, symbol)?;
    let path = metadata_string(&file.metadata, "path").unwrap_or_else(|| file.name.clone());
    let package = metadata_string(&file.metadata, "package").unwrap_or_else(|| "main".to_string());
    Some(scope_from_path(&path, &package))
}

fn nearest_file_symbol<'a>(
    base: &'a SemanticGraph,
    symbol: &'a SymbolNode,
) -> Option<&'a SymbolNode> {
    let mut current = symbol;
    loop {
        if current.kind == "file" {
            return Some(current);
        }
        current = base.symbols.get(&current.parent_id?)?;
    }
}

fn base_symbol(symbol: &SymbolNode) -> BaseSymbol {
    BaseSymbol {
        id: symbol.id,
        name: symbol.name.clone(),
        signature: metadata_string(&symbol.metadata, "signature").unwrap_or_default(),
        declaration: metadata_string(&symbol.metadata, "declaration").unwrap_or_default(),
        body: symbol.body.clone().unwrap_or_default(),
        doc: metadata_string(&symbol.metadata, "doc"),
    }
}
