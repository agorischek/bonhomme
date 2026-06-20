use crate::import::calls::{CallTarget, stable_reference_uuid};
use bonhomme_core::{Operation, ReferenceNode, SemanticGraph};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const CALLS_KIND: &str = "calls";

#[derive(Clone, Debug)]
pub(super) struct SymbolIdentity {
    pub(super) id: Uuid,
    pub(super) parent_id: Option<Uuid>,
    pub(super) name: String,
}

#[derive(Default, Debug)]
pub(super) struct ReferencePlan {
    pub(super) deleted_symbols: BTreeSet<Uuid>,
    pub(super) renamed_symbols: BTreeMap<Uuid, String>,
    pub(super) created_symbols: Vec<SymbolIdentity>,
    pub(super) edited_calls: BTreeMap<Uuid, Vec<CallTarget>>,
}

#[derive(Clone, Debug)]
struct EffectiveSymbol {
    id: Uuid,
    parent_id: Option<Uuid>,
    name: String,
    ordinal: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ReferenceEdge {
    id: Uuid,
    from_symbol_id: Uuid,
    to_symbol_id: Uuid,
}

#[derive(Default)]
struct ReferenceIndexes {
    symbols: BTreeMap<Uuid, EffectiveSymbol>,
    name_index: BTreeMap<String, Vec<Uuid>>,
    sibling_index: BTreeMap<(Uuid, String), Uuid>,
}

pub(super) fn recover_reference_operations(
    base: &SemanticGraph,
    plan: &ReferencePlan,
) -> (Vec<Operation>, Vec<Operation>) {
    let indexes = build_reference_indexes(base, plan);
    let desired_calls = desired_call_references(&indexes, plan);
    let deletes = reference_delete_operations(base, plan, &desired_calls);
    let creates = reference_create_operations(base, &desired_calls);

    (deletes, creates)
}

fn build_reference_indexes(base: &SemanticGraph, plan: &ReferencePlan) -> ReferenceIndexes {
    let mut symbols = BTreeMap::new();
    for symbol in base.symbols.values() {
        if symbol.kind == "file" || plan.deleted_symbols.contains(&symbol.id) {
            continue;
        }
        symbols.insert(
            symbol.id,
            EffectiveSymbol {
                id: symbol.id,
                parent_id: symbol.parent_id,
                name: plan
                    .renamed_symbols
                    .get(&symbol.id)
                    .cloned()
                    .unwrap_or_else(|| symbol.name.clone()),
                ordinal: symbol.ordinal,
            },
        );
    }

    let created_ordinal_start = base.symbols.len() as i64 + 1;
    for (index, symbol) in plan.created_symbols.iter().enumerate() {
        if plan.deleted_symbols.contains(&symbol.id) {
            continue;
        }
        symbols.insert(
            symbol.id,
            EffectiveSymbol {
                id: symbol.id,
                parent_id: symbol.parent_id,
                name: symbol.name.clone(),
                ordinal: created_ordinal_start + index as i64,
            },
        );
    }

    let mut ordered_symbols = symbols.values().collect::<Vec<_>>();
    ordered_symbols.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut name_index = BTreeMap::new();
    let mut sibling_index = BTreeMap::new();
    for symbol in ordered_symbols {
        name_index
            .entry(symbol.name.clone())
            .or_insert_with(Vec::new)
            .push(symbol.id);
        if let Some(parent_id) = symbol.parent_id {
            sibling_index
                .entry((parent_id, symbol.name.clone()))
                .or_insert(symbol.id);
        }
    }

    ReferenceIndexes {
        symbols,
        name_index,
        sibling_index,
    }
}

fn desired_call_references(
    indexes: &ReferenceIndexes,
    plan: &ReferencePlan,
) -> BTreeSet<ReferenceEdge> {
    let mut desired = BTreeSet::new();
    for (caller_id, calls) in &plan.edited_calls {
        let Some(caller) = indexes.symbols.get(caller_id) else {
            continue;
        };
        for call in calls {
            let Some(target_id) = resolve_call_target(indexes, caller, call) else {
                continue;
            };
            if target_id == *caller_id {
                continue;
            }
            desired.insert(ReferenceEdge {
                id: stable_reference_uuid(*caller_id, target_id, CALLS_KIND),
                from_symbol_id: *caller_id,
                to_symbol_id: target_id,
            });
        }
    }
    desired
}

fn resolve_call_target(
    indexes: &ReferenceIndexes,
    caller: &EffectiveSymbol,
    call: &CallTarget,
) -> Option<Uuid> {
    match call {
        CallTarget::This(name) => caller.parent_id.and_then(|parent_id| {
            indexes
                .sibling_index
                .get(&(parent_id, name.clone()))
                .copied()
        }),
        CallTarget::Free(name) => indexes
            .name_index
            .get(name)
            .and_then(|ids| (ids.len() == 1).then_some(ids[0])),
    }
}

fn reference_delete_operations(
    base: &SemanticGraph,
    plan: &ReferencePlan,
    desired_calls: &BTreeSet<ReferenceEdge>,
) -> Vec<Operation> {
    let desired_call_endpoints = desired_calls
        .iter()
        .map(|edge| (edge.from_symbol_id, edge.to_symbol_id))
        .collect::<BTreeSet<_>>();
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
            && !desired_call_endpoints.contains(&(reference.from_symbol_id, reference.to_symbol_id))
        {
            delete_ids.insert(reference.id);
        }
    }

    sorted_references(base, &delete_ids)
        .into_iter()
        .map(|reference| Operation::DeleteReference {
            reference_id: reference.id,
        })
        .collect()
}

fn reference_create_operations(
    base: &SemanticGraph,
    desired_calls: &BTreeSet<ReferenceEdge>,
) -> Vec<Operation> {
    let existing_call_endpoints = base
        .references
        .values()
        .filter(|reference| reference.kind == CALLS_KIND)
        .map(|reference| (reference.from_symbol_id, reference.to_symbol_id))
        .collect::<BTreeSet<_>>();

    desired_calls
        .iter()
        .filter(|edge| !existing_call_endpoints.contains(&(edge.from_symbol_id, edge.to_symbol_id)))
        .map(|edge| Operation::CreateReference {
            reference_id: edge.id,
            from_symbol_id: edge.from_symbol_id,
            to_symbol_id: edge.to_symbol_id,
            kind: CALLS_KIND.to_string(),
        })
        .collect()
}

fn sorted_references<'a>(
    base: &'a SemanticGraph,
    reference_ids: &BTreeSet<Uuid>,
) -> Vec<&'a ReferenceNode> {
    let mut references = reference_ids
        .iter()
        .filter_map(|reference_id| base.references.get(reference_id))
        .collect::<Vec<_>>();
    references.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.id.cmp(&right.id))
    });
    references
}
