use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use bonhomme_core::{
    Operation, ReferenceNode, RenderedFile, SemanticGraph, SymbolNode, metadata_string,
};
use uuid::Uuid;

use crate::{import::import_markdown_files, render::render_files};

pub fn recover_markdown_operations(
    base: &SemanticGraph,
    scope: &[Uuid],
    edited: &[RenderedFile],
) -> Result<Vec<Operation>> {
    let base_files = base_files_by_path(base, scope);
    let edited_paths = edited
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let desired_files = desired_files(base, &base_files, scope, edited);
    let desired_ops = import_markdown_files(&desired_files)?;
    let desired = graph_from_operations(&desired_ops)?;

    let mut plan = Plan::default();
    let target_symbols = target_symbol_ids(base, &base_files);
    let desired_symbols = desired.symbols.keys().copied().collect::<BTreeSet<_>>();
    let create_scope = create_scope_paths(scope, edited);

    delete_missing_files(base, &base_files, &edited_paths, &mut plan);
    delete_missing_symbols(base, &target_symbols, &desired_symbols, &mut plan);
    update_existing_symbols(base, &desired, &target_symbols, &mut plan);
    create_new_symbols(base, &desired, &desired_ops, &create_scope, &mut plan);
    refresh_references(base, &desired, &mut plan);

    Ok(plan.operations())
}

#[derive(Default)]
struct Plan {
    reference_deletes: Vec<Operation>,
    symbol_deletes: Vec<Operation>,
    symbol_updates: Vec<Operation>,
    symbol_creates: Vec<Operation>,
    reference_creates: Vec<Operation>,
    deleted_symbols: BTreeSet<Uuid>,
    changed_symbols: BTreeSet<Uuid>,
}

impl Plan {
    fn operations(self) -> Vec<Operation> {
        let mut operations = self.reference_deletes;
        operations.extend(self.symbol_deletes);
        operations.extend(self.symbol_updates);
        operations.extend(self.symbol_creates);
        operations.extend(self.reference_creates);
        operations
    }
}

fn desired_files(
    base: &SemanticGraph,
    base_files: &BTreeMap<String, &SymbolNode>,
    scope: &[Uuid],
    edited: &[RenderedFile],
) -> Vec<RenderedFile> {
    if scope.is_empty() {
        return edited.to_vec();
    }

    let edited_paths = edited
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let scoped_paths = base_files
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut files = edited.to_vec();

    for file in render_files(base) {
        if !edited_paths.contains(file.path.as_str()) && !scoped_paths.contains(file.path.as_str())
        {
            files.push(file);
        }
    }

    files
}

fn create_scope_paths(scope: &[Uuid], edited: &[RenderedFile]) -> Option<BTreeSet<String>> {
    if scope.is_empty() {
        return None;
    }
    Some(edited.iter().map(|file| file.path.clone()).collect())
}

fn graph_from_operations(operations: &[Operation]) -> Result<SemanticGraph> {
    let mut graph = SemanticGraph::default();
    for operation in operations {
        graph.apply_operation(Uuid::new_v4(), operation)?;
    }
    Ok(graph)
}

fn base_files_by_path<'a>(
    base: &'a SemanticGraph,
    scope: &[Uuid],
) -> BTreeMap<String, &'a SymbolNode> {
    let ids = if scope.is_empty() {
        base.root_symbols()
            .into_iter()
            .filter(|symbol| symbol.kind == "file")
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>()
    } else {
        scope
            .iter()
            .filter_map(|id| base.symbols.get(id))
            .filter_map(|symbol| nearest_file_symbol(base, symbol))
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>()
    };

    ids.into_iter()
        .filter_map(|id| base.symbols.get(&id))
        .map(|symbol| (file_path(symbol), symbol))
        .collect()
}

fn delete_missing_files(
    base: &SemanticGraph,
    base_files: &BTreeMap<String, &SymbolNode>,
    edited_paths: &BTreeSet<&str>,
    plan: &mut Plan,
) {
    for (path, file_symbol) in base_files {
        if !edited_paths.contains(path.as_str()) {
            delete_subtree(base, file_symbol.id, plan);
        }
    }
}

fn delete_missing_symbols(
    base: &SemanticGraph,
    target_symbols: &BTreeSet<Uuid>,
    desired_symbols: &BTreeSet<Uuid>,
    plan: &mut Plan,
) {
    for symbol_id in target_symbols {
        if !desired_symbols.contains(symbol_id) {
            delete_subtree(base, *symbol_id, plan);
        }
    }
}

fn update_existing_symbols(
    base: &SemanticGraph,
    desired: &SemanticGraph,
    target_symbols: &BTreeSet<Uuid>,
    plan: &mut Plan,
) {
    for (symbol_id, desired_symbol) in &desired.symbols {
        if !target_symbols.contains(symbol_id) {
            continue;
        }
        let Some(base_symbol) = base.symbols.get(symbol_id) else {
            continue;
        };
        if let Some(operation) = update_if_changed(base_symbol, desired_symbol) {
            plan.changed_symbols.insert(*symbol_id);
            plan.symbol_updates.push(operation);
        }
    }
}

fn create_new_symbols(
    base: &SemanticGraph,
    desired: &SemanticGraph,
    desired_ops: &[Operation],
    create_scope: &Option<BTreeSet<String>>,
    plan: &mut Plan,
) {
    for operation in desired_ops {
        if let Operation::CreateSymbol { symbol_id, .. } = operation
            && !base.symbols.contains_key(symbol_id)
            && is_in_create_scope(desired, *symbol_id, create_scope)
        {
            plan.changed_symbols.insert(*symbol_id);
            plan.symbol_creates.push(operation.clone());
        }
    }
}

fn is_in_create_scope(
    desired: &SemanticGraph,
    symbol_id: Uuid,
    create_scope: &Option<BTreeSet<String>>,
) -> bool {
    let Some(paths) = create_scope else {
        return true;
    };
    desired
        .symbols
        .get(&symbol_id)
        .and_then(|symbol| nearest_file_symbol(desired, symbol))
        .map(file_path)
        .is_some_and(|path| paths.contains(&path))
}

fn refresh_references(base: &SemanticGraph, desired: &SemanticGraph, plan: &mut Plan) {
    let desired_refs = desired.references.keys().copied().collect::<BTreeSet<_>>();
    for (reference_id, reference) in &base.references {
        if should_delete_reference(reference, &desired_refs, plan) {
            plan.reference_deletes.push(Operation::DeleteReference {
                reference_id: *reference_id,
            });
        }
    }
    for (reference_id, reference) in &desired.references {
        if !base.references.contains_key(reference_id)
            && plan.changed_symbols.contains(&reference.from_symbol_id)
        {
            plan.reference_creates.push(Operation::CreateReference {
                reference_id: *reference_id,
                from_symbol_id: reference.from_symbol_id,
                to_symbol_id: reference.to_symbol_id,
                kind: reference.kind.clone(),
            });
        }
    }
}

fn should_delete_reference(
    reference: &ReferenceNode,
    desired_refs: &BTreeSet<Uuid>,
    plan: &Plan,
) -> bool {
    (plan.deleted_symbols.contains(&reference.from_symbol_id)
        || plan.deleted_symbols.contains(&reference.to_symbol_id)
        || plan.changed_symbols.contains(&reference.from_symbol_id))
        && !desired_refs.contains(&reference.id)
}

fn update_if_changed(base: &SymbolNode, desired: &SymbolNode) -> Option<Operation> {
    let name = (base.name != desired.name).then(|| desired.name.clone());
    let body = (base.body != desired.body)
        .then(|| desired.body.clone())
        .flatten();
    let metadata = (base.metadata != desired.metadata).then(|| desired.metadata.clone());
    if name.is_none() && body.is_none() && metadata.is_none() {
        return None;
    }
    Some(Operation::UpdateSymbol {
        symbol_id: base.id,
        name,
        body,
        metadata,
    })
}

fn target_symbol_ids(
    base: &SemanticGraph,
    base_files: &BTreeMap<String, &SymbolNode>,
) -> BTreeSet<Uuid> {
    let mut ids = BTreeSet::new();
    for file_symbol in base_files.values() {
        collect_subtree_ids(base, file_symbol.id, &mut ids);
    }
    ids
}

fn delete_subtree(base: &SemanticGraph, symbol_id: Uuid, plan: &mut Plan) {
    if !plan.deleted_symbols.insert(symbol_id) {
        return;
    }
    for child in base.children_of(symbol_id) {
        delete_subtree(base, child.id, plan);
    }
    plan.changed_symbols.insert(symbol_id);
    plan.symbol_deletes
        .push(Operation::DeleteSymbol { symbol_id });
}

fn collect_subtree_ids(base: &SemanticGraph, symbol_id: Uuid, ids: &mut BTreeSet<Uuid>) {
    ids.insert(symbol_id);
    for child in base.children_of(symbol_id) {
        collect_subtree_ids(base, child.id, ids);
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

fn file_path(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone())
}
