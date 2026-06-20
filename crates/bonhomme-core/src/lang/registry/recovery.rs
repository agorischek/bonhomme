use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use uuid::Uuid;

use super::{
    HandlerRegistry, file_symbol_path, file_symbols, nearest_file_symbol, subgraph_for_files,
};
use crate::{
    core::{Operation, SemanticGraph},
    lang::RenderedFile,
};

impl HandlerRegistry {
    pub(super) fn recover_by_handlers(
        &self,
        base: &SemanticGraph,
        scope: &[Uuid],
        edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        let base = route_base_files(self, base);
        let edited_groups = route_edited_files(self, &base.handler_by_path, edited);
        let work = plan_recovery_work(self, &base, &edited_groups, scope);

        let mut operations = Vec::new();
        for (index, file_scope) in work {
            let file_ids = base
                .files_by_handler
                .get(&index)
                .cloned()
                .unwrap_or_default();
            let subgraph = subgraph_for_files(base.graph, &file_ids);
            let handler_scope: Vec<Uuid> = file_scope.into_iter().collect();
            let handler_edited = edited_groups.get(&index).cloned().unwrap_or_default();
            operations.extend(self.handlers[index].recover_operations(
                &subgraph,
                &handler_scope,
                &handler_edited,
            )?);
        }
        Ok(operations)
    }
}

struct BaseRouting<'a> {
    graph: &'a SemanticGraph,
    handler_by_path: BTreeMap<String, usize>,
    id_by_path: BTreeMap<String, Uuid>,
    files_by_handler: BTreeMap<usize, BTreeSet<Uuid>>,
}

fn route_base_files<'a>(registry: &HandlerRegistry, graph: &'a SemanticGraph) -> BaseRouting<'a> {
    let mut routing = BaseRouting {
        graph,
        handler_by_path: BTreeMap::new(),
        id_by_path: BTreeMap::new(),
        files_by_handler: BTreeMap::new(),
    };
    for file_symbol in file_symbols(graph) {
        let index = registry.handler_index_for_symbol(file_symbol);
        let path = file_symbol_path(file_symbol);
        routing.handler_by_path.insert(path.clone(), index);
        routing.id_by_path.insert(path, file_symbol.id);
        routing
            .files_by_handler
            .entry(index)
            .or_default()
            .insert(file_symbol.id);
    }
    routing
}

fn route_edited_files(
    registry: &HandlerRegistry,
    handler_by_path: &BTreeMap<String, usize>,
    edited: &[RenderedFile],
) -> BTreeMap<usize, Vec<RenderedFile>> {
    let mut groups = BTreeMap::new();
    for file in edited {
        let index = handler_by_path
            .get(&file.path)
            .copied()
            .unwrap_or_else(|| registry.claimant_index(file));
        groups
            .entry(index)
            .or_insert_with(Vec::new)
            .push(file.clone());
    }
    groups
}

fn plan_recovery_work(
    registry: &HandlerRegistry,
    base: &BaseRouting<'_>,
    edited_groups: &BTreeMap<usize, Vec<RenderedFile>>,
    scope: &[Uuid],
) -> BTreeMap<usize, BTreeSet<Uuid>> {
    if scope.is_empty() {
        return whole_repo_work(base, edited_groups);
    }
    focused_work(registry, base, edited_groups, scope)
}

fn whole_repo_work(
    base: &BaseRouting<'_>,
    edited_groups: &BTreeMap<usize, Vec<RenderedFile>>,
) -> BTreeMap<usize, BTreeSet<Uuid>> {
    let mut work = BTreeMap::new();
    for index in base.files_by_handler.keys().chain(edited_groups.keys()) {
        work.entry(*index).or_default();
    }
    work
}

fn focused_work(
    registry: &HandlerRegistry,
    base: &BaseRouting<'_>,
    edited_groups: &BTreeMap<usize, Vec<RenderedFile>>,
    scope: &[Uuid],
) -> BTreeMap<usize, BTreeSet<Uuid>> {
    let mut work = BTreeMap::new();
    include_scoped_files(registry, base, scope, &mut work);
    include_edited_base_files(base, edited_groups, &mut work);
    work
}

fn include_scoped_files(
    registry: &HandlerRegistry,
    base: &BaseRouting<'_>,
    scope: &[Uuid],
    work: &mut BTreeMap<usize, BTreeSet<Uuid>>,
) {
    for symbol_id in scope {
        if let Some(symbol) = base.graph.symbols.get(symbol_id)
            && let Some(file_symbol) = nearest_file_symbol(base.graph, symbol)
        {
            let index = registry.handler_index_for_symbol(file_symbol);
            work.entry(index).or_default().insert(file_symbol.id);
        }
    }
}

fn include_edited_base_files(
    base: &BaseRouting<'_>,
    edited_groups: &BTreeMap<usize, Vec<RenderedFile>>,
    work: &mut BTreeMap<usize, BTreeSet<Uuid>>,
) {
    for (index, files) in edited_groups {
        let entry = work.entry(*index).or_default();
        for file in files {
            if let Some(id) = base.id_by_path.get(&file.path) {
                entry.insert(*id);
            }
        }
    }
}
