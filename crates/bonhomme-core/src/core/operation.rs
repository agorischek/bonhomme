use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "PascalCase",
    rename_all_fields = "camelCase"
)]
pub enum Operation {
    CreateSymbol {
        symbol_id: Uuid,
        parent_id: Option<Uuid>,
        kind: String,
        name: String,
        body: Option<String>,
        metadata: Value,
    },
    DeleteSymbol {
        symbol_id: Uuid,
    },
    UpdateSymbol {
        symbol_id: Uuid,
        name: Option<String>,
        body: Option<String>,
        metadata: Option<Value>,
    },
    /// Re-parent a symbol while preserving its identity (id/name/body). This is how an
    /// identity-preserving *move* is expressed — a class to another file, a method to another class
    /// — since `UpdateSymbol` cannot change parentage. `new_parent_id` is `None` for top level.
    MoveSymbol {
        symbol_id: Uuid,
        new_parent_id: Option<Uuid>,
    },
    CreateReference {
        reference_id: Uuid,
        from_symbol_id: Uuid,
        to_symbol_id: Uuid,
        kind: String,
    },
    DeleteReference {
        reference_id: Uuid,
    },
}

/// Machine-readable replay conditions. These names are part of the semantic protocol: adding a
/// condition is backward-compatible, while renaming or changing one requires a protocol major
/// version. They mirror the checks in `SemanticGraph::apply_operation`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum OperationCondition {
    SymbolIdAbsent,
    ParentExistsIfPresent,
    SiblingNameUnique,
    SymbolExists,
    SymbolHasNoChildren,
    SymbolHasNoReferences,
    NewParentExistsIfPresent,
    MoveDoesNotCreateCycle,
    ReferenceIdAbsent,
    ReferenceEndpointsExist,
    ReferenceExists,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum OperationEffect {
    SymbolCreated,
    SymbolDeleted,
    SymbolUpdated,
    SymbolMoved,
    ReferenceCreated,
    ReferenceDeleted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationContract {
    pub preconditions: Vec<OperationCondition>,
    pub postconditions: Vec<OperationEffect>,
}

impl Operation {
    /// The graph invariants required and established by this operation. This is descriptive (the
    /// replay engine remains authoritative) and lets remote workers negotiate work without relying
    /// on error-message text.
    pub fn contract(&self) -> OperationContract {
        use OperationCondition as Condition;
        use OperationEffect as Effect;

        let (preconditions, postconditions) = match self {
            Operation::CreateSymbol { .. } => (
                vec![
                    Condition::SymbolIdAbsent,
                    Condition::ParentExistsIfPresent,
                    Condition::SiblingNameUnique,
                ],
                vec![Effect::SymbolCreated],
            ),
            Operation::DeleteSymbol { .. } => (
                vec![
                    Condition::SymbolExists,
                    Condition::SymbolHasNoChildren,
                    Condition::SymbolHasNoReferences,
                ],
                vec![Effect::SymbolDeleted],
            ),
            Operation::UpdateSymbol { .. } => (
                vec![Condition::SymbolExists, Condition::SiblingNameUnique],
                vec![Effect::SymbolUpdated],
            ),
            Operation::MoveSymbol { .. } => (
                vec![
                    Condition::SymbolExists,
                    Condition::NewParentExistsIfPresent,
                    Condition::MoveDoesNotCreateCycle,
                    Condition::SiblingNameUnique,
                ],
                vec![Effect::SymbolMoved],
            ),
            Operation::CreateReference { .. } => (
                vec![
                    Condition::ReferenceIdAbsent,
                    Condition::ReferenceEndpointsExist,
                ],
                vec![Effect::ReferenceCreated],
            ),
            Operation::DeleteReference { .. } => (
                vec![Condition::ReferenceExists],
                vec![Effect::ReferenceDeleted],
            ),
        };
        OperationContract {
            preconditions,
            postconditions,
        }
    }

    pub fn op_type(&self) -> &'static str {
        match self {
            Operation::CreateSymbol { .. } => "CreateSymbol",
            Operation::DeleteSymbol { .. } => "DeleteSymbol",
            Operation::UpdateSymbol { .. } => "UpdateSymbol",
            Operation::MoveSymbol { .. } => "MoveSymbol",
            Operation::CreateReference { .. } => "CreateReference",
            Operation::DeleteReference { .. } => "DeleteReference",
        }
    }

    pub fn created_symbol_id(&self) -> Option<Uuid> {
        match self {
            Operation::CreateSymbol { symbol_id, .. } => Some(*symbol_id),
            _ => None,
        }
    }

    pub fn created_symbol_key(&self) -> Option<SymbolNameKey> {
        match self {
            Operation::CreateSymbol {
                parent_id,
                kind,
                name,
                ..
            } => Some(SymbolNameKey {
                parent_id: *parent_id,
                kind: kind.clone(),
                name: name.clone(),
            }),
            _ => None,
        }
    }

    pub fn write_symbols(&self) -> BTreeSet<Uuid> {
        let mut ids = BTreeSet::new();
        match self {
            Operation::CreateSymbol { symbol_id, .. }
            | Operation::DeleteSymbol { symbol_id }
            | Operation::UpdateSymbol { symbol_id, .. }
            | Operation::MoveSymbol { symbol_id, .. } => {
                ids.insert(*symbol_id);
            }
            Operation::CreateReference { .. } => {}
            Operation::DeleteReference { .. } => {}
        }
        ids
    }

    pub fn created_reference_id(&self) -> Option<Uuid> {
        match self {
            Operation::CreateReference { reference_id, .. } => Some(*reference_id),
            _ => None,
        }
    }

    pub fn deleted_symbol_id(&self) -> Option<Uuid> {
        match self {
            Operation::DeleteSymbol { symbol_id } => Some(*symbol_id),
            _ => None,
        }
    }

    pub fn reference_endpoints(&self) -> Option<(Uuid, Uuid)> {
        match self {
            Operation::CreateReference {
                from_symbol_id,
                to_symbol_id,
                ..
            } => Some((*from_symbol_id, *to_symbol_id)),
            _ => None,
        }
    }

    /// The reference id that this operation creates or deletes, if any. Mirrors
    /// [`write_symbols`] so the merge analyzer can treat reference edits as a write set.
    pub fn write_references(&self) -> Option<Uuid> {
        match self {
            Operation::CreateReference { reference_id, .. }
            | Operation::DeleteReference { reference_id } => Some(*reference_id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct SymbolNameKey {
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
}

#[cfg(test)]
mod protocol_tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{Operation, OperationCondition, OperationEffect};

    #[test]
    fn operation_wire_shape_and_contract_are_stable() {
        let operation = Operation::MoveSymbol {
            symbol_id: Uuid::nil(),
            new_parent_id: Some(Uuid::from_u128(1)),
        };
        assert_eq!(
            serde_json::to_value(&operation).unwrap(),
            json!({
                "type": "MoveSymbol",
                "symbolId": Uuid::nil(),
                "newParentId": Uuid::from_u128(1),
            })
        );
        let contract = operation.contract();
        assert_eq!(
            contract.preconditions,
            vec![
                OperationCondition::SymbolExists,
                OperationCondition::NewParentExistsIfPresent,
                OperationCondition::MoveDoesNotCreateCycle,
                OperationCondition::SiblingNameUnique,
            ]
        );
        assert_eq!(contract.postconditions, vec![OperationEffect::SymbolMoved]);
    }
}
