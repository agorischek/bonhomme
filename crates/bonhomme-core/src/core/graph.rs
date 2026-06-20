use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use super::{Operation, OperationRecord, ReferenceNode, SymbolNameKey, SymbolNode};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SemanticGraph {
    pub symbols: BTreeMap<Uuid, SymbolNode>,
    pub references: BTreeMap<Uuid, ReferenceNode>,
    pub applied_operations: Vec<Uuid>,
}

impl SemanticGraph {
    pub fn apply_record(&mut self, record: &OperationRecord) -> Result<()> {
        self.apply_operation(record.id, &record.operation)
    }

    pub fn apply_operation(&mut self, operation_id: Uuid, operation: &Operation) -> Result<()> {
        let ordinal = self.applied_operations.len() as i64 + 1;

        match operation {
            Operation::CreateSymbol {
                symbol_id,
                parent_id,
                kind,
                name,
                body,
                metadata,
            } => {
                if self.symbols.contains_key(symbol_id) {
                    bail!("duplicate symbol id {symbol_id}");
                }
                if let Some(parent_id) = parent_id
                    && !self.symbols.contains_key(parent_id)
                {
                    bail!("parent symbol {parent_id} does not exist");
                }
                if self.has_symbol_named(*parent_id, kind, name, None) {
                    bail!("duplicate {kind} symbol named {name}");
                }

                self.symbols.insert(
                    *symbol_id,
                    SymbolNode {
                        id: *symbol_id,
                        parent_id: *parent_id,
                        kind: kind.clone(),
                        name: name.clone(),
                        body: body.clone(),
                        metadata: metadata.clone(),
                        ordinal,
                    },
                );
            }
            Operation::DeleteSymbol { symbol_id } => {
                if !self.symbols.contains_key(symbol_id) {
                    bail!("cannot delete missing symbol {symbol_id}");
                }
                if self
                    .symbols
                    .values()
                    .any(|symbol| symbol.parent_id == Some(*symbol_id))
                {
                    bail!("cannot delete symbol {symbol_id} while it still contains children");
                }
                if self.references.values().any(|reference| {
                    reference.from_symbol_id == *symbol_id || reference.to_symbol_id == *symbol_id
                }) {
                    bail!("cannot delete symbol {symbol_id} while references still point at it");
                }
                self.symbols.remove(symbol_id);
            }
            Operation::UpdateSymbol {
                symbol_id,
                name,
                body,
                metadata,
            } => {
                let current =
                    self.symbols.get(symbol_id).cloned().ok_or_else(|| {
                        anyhow::anyhow!("cannot update missing symbol {symbol_id}")
                    })?;
                if let Some(name) = name
                    && self.has_symbol_named(
                        current.parent_id,
                        &current.kind,
                        name,
                        Some(*symbol_id),
                    )
                {
                    bail!("duplicate {} symbol named {}", current.kind, name);
                }

                let symbol = self.symbols.get_mut(symbol_id).expect("checked above");
                if let Some(name) = name {
                    symbol.name = name.clone();
                }
                if let Some(body) = body {
                    symbol.body = Some(body.clone());
                }
                if let Some(metadata) = metadata {
                    symbol.metadata = metadata.clone();
                }
            }
            Operation::CreateReference {
                reference_id,
                from_symbol_id,
                to_symbol_id,
                kind,
            } => {
                if self.references.contains_key(reference_id) {
                    bail!("duplicate reference id {reference_id}");
                }
                if !self.symbols.contains_key(from_symbol_id) {
                    bail!("reference source symbol {from_symbol_id} does not exist");
                }
                if !self.symbols.contains_key(to_symbol_id) {
                    bail!("reference target symbol {to_symbol_id} does not exist");
                }
                self.references.insert(
                    *reference_id,
                    ReferenceNode {
                        id: *reference_id,
                        from_symbol_id: *from_symbol_id,
                        to_symbol_id: *to_symbol_id,
                        kind: kind.clone(),
                        ordinal,
                    },
                );
            }
            Operation::DeleteReference { reference_id } => {
                if self.references.remove(reference_id).is_none() {
                    bail!("cannot delete missing reference {reference_id}");
                }
            }
        }

        self.applied_operations.push(operation_id);
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut symbol_keys = BTreeSet::new();
        for symbol in self.symbols.values() {
            if let Some(parent_id) = symbol.parent_id
                && !self.symbols.contains_key(&parent_id)
            {
                bail!("symbol {} has dangling parent {parent_id}", symbol.id);
            }
            let key = SymbolNameKey {
                parent_id: symbol.parent_id,
                kind: symbol.kind.clone(),
                name: symbol.name.clone(),
            };
            if !symbol_keys.insert(key) {
                bail!("duplicate sibling symbol detected");
            }
        }

        for reference in self.references.values() {
            if !self.symbols.contains_key(&reference.from_symbol_id) {
                bail!(
                    "reference {} has dangling source {}",
                    reference.id,
                    reference.from_symbol_id
                );
            }
            if !self.symbols.contains_key(&reference.to_symbol_id) {
                bail!(
                    "reference {} has dangling target {}",
                    reference.id,
                    reference.to_symbol_id
                );
            }
        }

        Ok(())
    }

    fn has_symbol_named(
        &self,
        parent_id: Option<Uuid>,
        kind: &str,
        name: &str,
        exclude_id: Option<Uuid>,
    ) -> bool {
        self.symbols.values().any(|symbol| {
            symbol.parent_id == parent_id
                && symbol.kind == kind
                && symbol.name == name
                && Some(symbol.id) != exclude_id
        })
    }
}

pub fn materialize(records: &[OperationRecord]) -> Result<SemanticGraph> {
    let mut graph = SemanticGraph::default();
    for record in records {
        graph.apply_record(record)?;
    }
    // Per-op create/delete checks maintain every invariant incrementally; a single
    // full validation at the end confirms the result without paying O(n^2) per replay.
    graph.validate()?;
    Ok(graph)
}
