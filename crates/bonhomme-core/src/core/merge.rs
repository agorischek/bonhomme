use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Operation, OperationRecord};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MergeOutcome {
    SafeMerge,
    Conflict,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflict {
    pub reason: String,
    pub source_operation_id: Uuid,
    pub target_operation_id: Option<Uuid>,
    pub symbol_id: Option<Uuid>,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeAnalysis {
    pub outcome: MergeOutcome,
    pub conflicts: Vec<MergeConflict>,
}

pub fn analyze_merge(
    target_since_base: &[OperationRecord],
    source_operations: &[OperationRecord],
) -> MergeAnalysis {
    let mut conflicts = Vec::new();

    for source in source_operations {
        for target in target_since_base {
            if let (Some(source_id), Some(target_id)) = (
                source.operation.created_symbol_id(),
                target.operation.created_symbol_id(),
            ) && source_id == target_id
            {
                conflicts.push(MergeConflict {
                    reason: "DUPLICATE_SYMBOL_ID".to_string(),
                    source_operation_id: source.id,
                    target_operation_id: Some(target.id),
                    symbol_id: Some(source_id),
                    detail: format!("both branches create symbol {source_id}"),
                });
            }

            if let (Some(source_id), Some(target_id)) = (
                source.operation.created_reference_id(),
                target.operation.created_reference_id(),
            ) && source_id == target_id
            {
                conflicts.push(MergeConflict {
                    reason: "DUPLICATE_REFERENCE_ID".to_string(),
                    source_operation_id: source.id,
                    target_operation_id: Some(target.id),
                    symbol_id: None,
                    detail: format!("both branches create reference {source_id}"),
                });
            }

            if let (Some(source_key), Some(target_key)) = (
                source.operation.created_symbol_key(),
                target.operation.created_symbol_key(),
            ) && source_key == target_key
            {
                conflicts.push(MergeConflict {
                    reason: "DUPLICATE_SYMBOL_NAME".to_string(),
                    source_operation_id: source.id,
                    target_operation_id: Some(target.id),
                    symbol_id: source.operation.created_symbol_id(),
                    detail: format!(
                        "both branches create {} named {} under {:?}",
                        source_key.kind, source_key.name, source_key.parent_id
                    ),
                });
            }

            let source_writes = source.operation.write_symbols();
            let target_writes = target.operation.write_symbols();
            if let Some(symbol_id) = source_writes.intersection(&target_writes).copied().next() {
                let source_create = source.operation.created_symbol_id() == Some(symbol_id);
                let target_create = target.operation.created_symbol_id() == Some(symbol_id);
                if !(source_create && target_create) {
                    conflicts.push(MergeConflict {
                        reason: "OVERLAPPING_SYMBOL_WRITE".to_string(),
                        source_operation_id: source.id,
                        target_operation_id: Some(target.id),
                        symbol_id: Some(symbol_id),
                        detail: format!("both branches write symbol {symbol_id}"),
                    });
                }
            }

            if let Some(reference_id) =
                overlapping_reference_write(&source.operation, &target.operation)
            {
                conflicts.push(MergeConflict {
                    reason: "OVERLAPPING_REFERENCE_WRITE".to_string(),
                    source_operation_id: source.id,
                    target_operation_id: Some(target.id),
                    symbol_id: None,
                    detail: format!("both branches write reference {reference_id}"),
                });
            }

            if let Some(symbol_id) =
                reference_to_deleted_symbol(&source.operation, &target.operation)
            {
                conflicts.push(MergeConflict {
                    reason: "REFERENCE_TO_DELETED_SYMBOL".to_string(),
                    source_operation_id: source.id,
                    target_operation_id: Some(target.id),
                    symbol_id: Some(symbol_id),
                    detail: format!(
                        "one branch references symbol {symbol_id} that the other branch deletes"
                    ),
                });
            }
        }
    }

    conflicts.sort_by(|a, b| {
        a.reason
            .cmp(&b.reason)
            .then_with(|| a.source_operation_id.cmp(&b.source_operation_id))
            .then_with(|| a.target_operation_id.cmp(&b.target_operation_id))
    });
    conflicts.dedup();

    MergeAnalysis {
        outcome: if conflicts.is_empty() {
            MergeOutcome::SafeMerge
        } else {
            MergeOutcome::Conflict
        },
        conflicts,
    }
}

/// Two operations conflict when both touch the same reference id and not both merely create it
/// (e.g. both delete it, or one creates while the other deletes). Mirrors the symbol write-set
/// check so reference edits are caught up front instead of only at replay.
fn overlapping_reference_write(source: &Operation, target: &Operation) -> Option<Uuid> {
    let source_ref = source.write_references()?;
    let target_ref = target.write_references()?;
    if source_ref != target_ref {
        return None;
    }
    let both_create = source.created_reference_id() == Some(source_ref)
        && target.created_reference_id() == Some(target_ref);
    (!both_create).then_some(source_ref)
}

/// One branch creating a reference whose endpoint the other branch deletes always yields a
/// dangling reference after merge. Detect it statically rather than relying on replay to bail.
fn reference_to_deleted_symbol(source: &Operation, target: &Operation) -> Option<Uuid> {
    fn check(reference: &Operation, delete: &Operation) -> Option<Uuid> {
        let (from, to) = reference.reference_endpoints()?;
        let deleted = delete.deleted_symbol_id()?;
        (from == deleted || to == deleted).then_some(deleted)
    }
    check(source, target).or_else(|| check(target, source))
}
