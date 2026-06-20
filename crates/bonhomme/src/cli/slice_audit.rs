use bonhomme_core::{Operation, OperationRecord};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) struct SliceAuditContext {
    pub(super) slice_id: Uuid,
    pub(super) base_position: i64,
    pub(super) branch_position_at_apply: i64,
    pub(super) root_symbols: Vec<Uuid>,
}

pub(super) fn slice_recovery_audit(
    context: &SliceAuditContext,
    branch_name: &str,
    operations: &[Operation],
    appended: &[OperationRecord],
) -> Value {
    json!({
        "strategy": "stored-slice-recovery",
        "sliceId": context.slice_id,
        "branch": branch_name,
        "basePosition": context.base_position,
        "branchPositionAtApply": context.branch_position_at_apply,
        "stale": context.branch_position_at_apply != context.base_position,
        "rootSymbols": context.root_symbols,
        "operationCount": operations.len(),
        "appendedOperationIds": appended.iter().map(|record| record.id).collect::<Vec<_>>(),
        "decisions": operations.iter().map(operation_decision).collect::<Vec<_>>()
    })
}

fn operation_decision(operation: &Operation) -> Value {
    match operation {
        Operation::CreateSymbol {
            symbol_id,
            parent_id,
            kind,
            name,
            ..
        } => json!({
            "decision": "addedSymbol",
            "operation": "CreateSymbol",
            "symbolId": symbol_id,
            "parentId": parent_id,
            "kind": kind,
            "name": name
        }),
        Operation::DeleteSymbol { symbol_id } => json!({
            "decision": "deletedSymbol",
            "operation": "DeleteSymbol",
            "symbolId": symbol_id
        }),
        Operation::MoveSymbol {
            symbol_id,
            new_parent_id,
        } => json!({
            "decision": "movedSymbol",
            "operation": "MoveSymbol",
            "symbolId": symbol_id,
            "newParentId": new_parent_id
        }),
        Operation::UpdateSymbol {
            symbol_id,
            name,
            body,
            metadata,
        } => json!({
            "decision": if name.is_some() { "renamedSymbol" } else { "updatedSymbol" },
            "operation": "UpdateSymbol",
            "symbolId": symbol_id,
            "name": name,
            "bodyChanged": body.is_some(),
            "metadataChanged": metadata.is_some()
        }),
        Operation::CreateReference {
            reference_id,
            from_symbol_id,
            to_symbol_id,
            kind,
        } => json!({
            "decision": "addedReference",
            "operation": "CreateReference",
            "referenceId": reference_id,
            "fromSymbolId": from_symbol_id,
            "toSymbolId": to_symbol_id,
            "kind": kind
        }),
        Operation::DeleteReference { reference_id } => json!({
            "decision": "deletedReference",
            "operation": "DeleteReference",
            "referenceId": reference_id
        }),
    }
}
