use super::Plan;
use crate::import::{ImportIndexes, stable_reference_uuid};
use bonhomme_core::{Operation, SemanticGraph, metadata_string};
use std::collections::BTreeSet;
use uuid::Uuid;

const CALLS_KIND: &str = "calls";

pub(super) fn recover_references(base: &SemanticGraph, plan: &mut Plan) {
    let indexes = effective_indexes(base, plan);
    let desired = desired_references(&indexes, plan);
    let desired_endpoints = desired
        .iter()
        .map(|(_, from, to)| (*from, *to))
        .collect::<BTreeSet<_>>();
    let delete_ids = reference_delete_ids(base, plan, &desired_endpoints);

    plan.reference_deletes
        .extend(sorted_delete_operations(base, &delete_ids));
    plan.reference_creates
        .extend(reference_create_operations(base, desired));
}

fn reference_delete_ids(
    base: &SemanticGraph,
    plan: &Plan,
    desired_endpoints: &BTreeSet<(Uuid, Uuid)>,
) -> BTreeSet<Uuid> {
    let edited_callers = plan.edited_calls.keys().copied().collect::<BTreeSet<_>>();
    let mut delete_ids = BTreeSet::new();

    for reference in base.references.values() {
        if plan.deleted_symbols.contains(&reference.from_symbol_id)
            || plan.deleted_symbols.contains(&reference.to_symbol_id)
        {
            delete_ids.insert(reference.id);
            continue;
        }
        if reference.kind == CALLS_KIND
            && edited_callers.contains(&reference.from_symbol_id)
            && !desired_endpoints.contains(&(reference.from_symbol_id, reference.to_symbol_id))
        {
            delete_ids.insert(reference.id);
        }
    }

    delete_ids
}

fn sorted_delete_operations(base: &SemanticGraph, delete_ids: &BTreeSet<Uuid>) -> Vec<Operation> {
    let mut references = delete_ids
        .iter()
        .filter_map(|id| base.references.get(id))
        .collect::<Vec<_>>();
    references.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.id.cmp(&right.id))
    });
    references
        .into_iter()
        .map(|reference| Operation::DeleteReference {
            reference_id: reference.id,
        })
        .collect()
}

fn reference_create_operations(
    base: &SemanticGraph,
    desired: Vec<(Uuid, Uuid, Uuid)>,
) -> Vec<Operation> {
    let existing = base
        .references
        .values()
        .filter(|reference| reference.kind == CALLS_KIND)
        .map(|reference| (reference.from_symbol_id, reference.to_symbol_id))
        .collect::<BTreeSet<_>>();

    desired
        .into_iter()
        .filter(|(_, from_symbol_id, to_symbol_id)| {
            !existing.contains(&(*from_symbol_id, *to_symbol_id))
        })
        .map(
            |(reference_id, from_symbol_id, to_symbol_id)| Operation::CreateReference {
                reference_id,
                from_symbol_id,
                to_symbol_id,
                kind: CALLS_KIND.to_string(),
            },
        )
        .collect()
}

fn effective_indexes(base: &SemanticGraph, plan: &Plan) -> ImportIndexes {
    let mut indexes = ImportIndexes::default();
    for symbol in base.symbols.values() {
        if plan.deleted_symbols.contains(&symbol.id) {
            continue;
        }
        match symbol.kind.as_str() {
            "struct" | "interface" | "type" => {
                indexes.types.insert(symbol.name.clone(), symbol.id);
            }
            "function" => {
                indexes.functions.insert(symbol.name.clone(), symbol.id);
            }
            "method" if symbol.body.is_some() => {
                if let Some(receiver) = metadata_string(&symbol.metadata, "receiver") {
                    indexes
                        .methods
                        .insert((receiver, symbol.name.clone()), symbol.id);
                }
            }
            _ => {}
        }
    }
    add_created_symbols(base, plan, &mut indexes);
    indexes
}

fn add_created_symbols(base: &SemanticGraph, plan: &Plan, indexes: &mut ImportIndexes) {
    for (id, parent_id, kind, name) in &plan.created_symbols {
        match kind.as_str() {
            "function" => {
                indexes.functions.insert(name.clone(), *id);
            }
            "method" => {
                let receiver = parent_id
                    .and_then(|parent_id| base.symbols.get(&parent_id))
                    .map(|symbol| symbol.name.clone());
                if let Some(receiver) = receiver {
                    indexes.methods.insert((receiver, name.clone()), *id);
                }
            }
            _ => {}
        }
    }
}

fn desired_references(indexes: &ImportIndexes, plan: &Plan) -> Vec<(Uuid, Uuid, Uuid)> {
    let mut desired = BTreeSet::new();
    for (from_symbol_id, calls) in &plan.edited_calls {
        for call in calls {
            let Some(to_symbol_id) = crate::import::resolve_call(indexes, call) else {
                continue;
            };
            if to_symbol_id == *from_symbol_id {
                continue;
            }
            desired.insert((
                stable_reference_uuid(*from_symbol_id, to_symbol_id, CALLS_KIND),
                *from_symbol_id,
                to_symbol_id,
            ));
        }
    }
    desired.into_iter().collect()
}
