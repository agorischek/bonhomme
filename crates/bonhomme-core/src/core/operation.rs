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

impl Operation {
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
