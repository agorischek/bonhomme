use uuid::Uuid;

use super::{ReferenceNode, SemanticGraph, SymbolNode};

impl SemanticGraph {
    pub fn children_of(&self, parent_id: Uuid) -> Vec<&SymbolNode> {
        let mut children = self
            .symbols
            .values()
            .filter(|symbol| symbol.parent_id == Some(parent_id))
            .collect::<Vec<_>>();
        children.sort_by(|a, b| {
            a.ordinal
                .cmp(&b.ordinal)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        children
    }

    pub fn root_symbols(&self) -> Vec<&SymbolNode> {
        let mut roots = self
            .symbols
            .values()
            .filter(|symbol| symbol.parent_id.is_none())
            .collect::<Vec<_>>();
        roots.sort_by(|a, b| {
            a.ordinal
                .cmp(&b.ordinal)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        roots
    }

    pub fn find_symbol(&self, name: &str) -> Vec<&SymbolNode> {
        let mut symbols = self
            .symbols
            .values()
            .filter(|symbol| symbol.name == name)
            .collect::<Vec<_>>();
        sort_symbols_by_ordinal(&mut symbols);
        symbols
    }

    pub fn find_references(&self, symbol_id: Uuid) -> Vec<&ReferenceNode> {
        let mut references = self
            .references
            .values()
            .filter(|reference| {
                reference.from_symbol_id == symbol_id || reference.to_symbol_id == symbol_id
            })
            .collect::<Vec<_>>();
        references.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)));
        references
    }

    /// Symbols that reference `symbol_id` through an edge of the given `kind`. The kind is a
    /// parameter (rather than a hard-coded "calls") so the core holds no language-specific
    /// relationship vocabulary — that lives in the language plugin / caller.
    pub fn find_callers(&self, symbol_id: Uuid, kind: &str) -> Vec<&SymbolNode> {
        let mut callers = self
            .references
            .values()
            .filter(|reference| reference.kind == kind && reference.to_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.from_symbol_id))
            .collect::<Vec<_>>();
        sort_symbols_by_ordinal(&mut callers);
        callers.dedup_by_key(|symbol| symbol.id);
        callers
    }

    /// Symbols that `symbol_id` references through an edge of the given `kind`.
    pub fn find_callees(&self, symbol_id: Uuid, kind: &str) -> Vec<&SymbolNode> {
        let mut callees = self
            .references
            .values()
            .filter(|reference| reference.kind == kind && reference.from_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.to_symbol_id))
            .collect::<Vec<_>>();
        sort_symbols_by_ordinal(&mut callees);
        callees.dedup_by_key(|symbol| symbol.id);
        callees
    }

    pub fn find_dependencies(&self, symbol_id: Uuid) -> Vec<&SymbolNode> {
        let mut dependencies = self
            .references
            .values()
            .filter(|reference| reference.from_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.to_symbol_id))
            .collect::<Vec<_>>();
        sort_symbols_by_ordinal(&mut dependencies);
        dependencies.dedup_by_key(|symbol| symbol.id);
        dependencies
    }

    pub fn find_dependents(&self, symbol_id: Uuid) -> Vec<&SymbolNode> {
        let mut dependents = self
            .references
            .values()
            .filter(|reference| reference.to_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.from_symbol_id))
            .collect::<Vec<_>>();
        sort_symbols_by_ordinal(&mut dependents);
        dependents.dedup_by_key(|symbol| symbol.id);
        dependents
    }
}

/// Sort symbol references by creation order (ordinal) with a stable id tiebreak, so every
/// query method surfaces results in source order rather than arbitrary `Uuid` key order.
fn sort_symbols_by_ordinal(symbols: &mut [&SymbolNode]) {
    symbols.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)));
}
