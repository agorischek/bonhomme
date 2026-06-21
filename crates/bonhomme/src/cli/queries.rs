use anyhow::{Context, Result};
use bonhomme_core::{SemanticGraph, SymbolNode};
use bonhomme_engine::Storage;

use super::FindSymbolArgs;

/// The reference-edge kind the TypeScript plugin uses for call relationships. Lives in the
/// TS-aware CLI layer rather than `core`, which is language-agnostic.
const CALL_REFERENCE_KIND: &str = "calls";

pub(super) fn select_callers(graph: &SemanticGraph, symbol_id: uuid::Uuid) -> Vec<&SymbolNode> {
    graph.find_callers(symbol_id, CALL_REFERENCE_KIND)
}

pub(super) fn select_callees(graph: &SemanticGraph, symbol_id: uuid::Uuid) -> Vec<&SymbolNode> {
    graph.find_callees(symbol_id, CALL_REFERENCE_KIND)
}

pub(super) fn select_dependencies(
    graph: &SemanticGraph,
    symbol_id: uuid::Uuid,
) -> Vec<&SymbolNode> {
    graph.find_dependencies(symbol_id)
}

pub(super) fn select_dependents(graph: &SemanticGraph, symbol_id: uuid::Uuid) -> Vec<&SymbolNode> {
    graph.find_dependents(symbol_id)
}

/// Shared driver for the relationship queries that resolve a symbol by name and print the symbols
/// it relates to (callers/callees/dependencies/dependents), passed as a graph method.
pub(super) async fn print_related_symbols(
    storage: &Storage,
    repo: &str,
    args: &FindSymbolArgs,
    select: fn(&SemanticGraph, uuid::Uuid) -> Vec<&SymbolNode>,
) -> Result<()> {
    let materialized = storage.materialize_branch(repo, &args.branch).await?;
    let symbol = resolve_symbol(&materialized.graph, &args.name)?;
    let related = select(&materialized.graph, symbol.id);
    println!("{}", serde_json::to_string_pretty(&related)?);
    Ok(())
}

pub(super) fn resolve_symbol<'g>(graph: &'g SemanticGraph, name: &str) -> Result<&'g SymbolNode> {
    graph
        .find_symbol(name)
        .first()
        .copied()
        .with_context(|| format!("symbol {name} not found"))
}
