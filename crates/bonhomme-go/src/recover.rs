mod base;
mod files;
mod functions;
mod matcher;
mod references;
mod types;

use self::{
    base::base_file_paths,
    files::{recover_file_metadata, recover_top_level_values},
    functions::{recover_functions, recover_methods},
    references::recover_references,
    types::recover_types,
};
use crate::{model::CallTarget, toolchain::parse_go_files};
use anyhow::{Context, Result};
use bonhomme_core::{Operation, RenderedFile, SemanticGraph};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct Plan {
    pub(super) reference_deletes: Vec<Operation>,
    pub(super) symbol_deletes: Vec<Operation>,
    pub(super) symbol_edits: Vec<Operation>,
    pub(super) reference_creates: Vec<Operation>,
    pub(super) deleted_symbols: BTreeSet<Uuid>,
    pub(super) edited_calls: BTreeMap<Uuid, Vec<CallTarget>>,
    pub(super) created_symbols: Vec<(Uuid, Option<Uuid>, String, String)>,
}

pub fn recover_go_operations(
    base: &SemanticGraph,
    _scope: &[Uuid],
    edited: &[RenderedFile],
) -> Result<Vec<Operation>> {
    let parsed = parse_go_files(edited)?;
    let mut base_files = base_file_paths(base);
    let mut plan = Plan::default();

    for file in &parsed.files {
        let Some(base_file_id) = base_files.remove(&file.path) else {
            plan.symbol_edits.extend(import_new_file(file, edited)?);
            continue;
        };

        let base_file = base
            .symbols
            .get(&base_file_id)
            .with_context(|| format!("base Go file {} disappeared", file.path))?;
        recover_file_metadata(base_file, file, &mut plan);
        recover_top_level_values(base, base_file.id, file, &mut plan)?;
        recover_types(base, base_file.id, file, &mut plan)?;
        recover_functions(base, base_file.id, file, &mut plan)?;
    }

    recover_methods(base, &parsed.files, &mut plan)?;
    recover_references(base, &mut plan);
    Ok(planned_operations(plan))
}

fn import_new_file(
    file: &crate::model::ParsedFile,
    edited: &[RenderedFile],
) -> Result<Vec<Operation>> {
    let source = RenderedFile {
        path: file.path.clone(),
        content: edited
            .iter()
            .find(|candidate| candidate.path == file.path)
            .map(|candidate| candidate.content.clone())
            .unwrap_or_default(),
    };
    crate::import::import_go_files(std::slice::from_ref(&source))
}

fn planned_operations(plan: Plan) -> Vec<Operation> {
    let mut operations = plan.reference_deletes;
    operations.extend(plan.symbol_deletes);
    operations.extend(plan.symbol_edits);
    operations.extend(plan.reference_creates);
    operations
}

pub(super) fn delete_subtree(base: &SemanticGraph, symbol_id: Uuid, plan: &mut Plan) {
    for child in base.children_of(symbol_id) {
        delete_subtree(base, child.id, plan);
    }
    queue_delete(symbol_id, plan);
}

pub(super) fn queue_delete(symbol_id: Uuid, plan: &mut Plan) {
    plan.deleted_symbols.insert(symbol_id);
    plan.symbol_deletes
        .push(Operation::DeleteSymbol { symbol_id });
}
