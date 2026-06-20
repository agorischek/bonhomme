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

pub(super) fn base_type_by_name(base: &SemanticGraph, name: &str) -> Option<BaseSymbol> {
    base.symbols
        .values()
        .find(|symbol| {
            matches!(symbol.kind.as_str(), "struct" | "interface" | "type") && symbol.name == name
        })
        .map(base_symbol)
}

fn base_symbol(symbol: &SymbolNode) -> BaseSymbol {
    BaseSymbol {
        id: symbol.id,
        name: symbol.name.clone(),
        signature: metadata_string(&symbol.metadata, "signature").unwrap_or_default(),
        declaration: metadata_string(&symbol.metadata, "declaration").unwrap_or_default(),
        body: symbol.body.clone().unwrap_or_default(),
    }
}
