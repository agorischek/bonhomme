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
            Operation::MoveSymbol {
                symbol_id,
                new_parent_id,
            } => {
                let moved = self
                    .symbols
                    .get(symbol_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("cannot move missing symbol {symbol_id}"))?;
                if let Some(new_parent_id) = new_parent_id {
                    if new_parent_id == symbol_id {
                        bail!("cannot move symbol {symbol_id} into itself");
                    }
                    if !self.symbols.contains_key(new_parent_id) {
                        bail!("new parent symbol {new_parent_id} does not exist");
                    }
                    // Walk up from the proposed parent; reaching the moved symbol means the move
                    // would place it beneath its own descendant — a cycle.
                    let mut ancestor = Some(*new_parent_id);
                    while let Some(current) = ancestor {
                        if current == *symbol_id {
                            bail!("cannot move symbol {symbol_id} beneath its own descendant");
                        }
                        ancestor = self.symbols.get(&current).and_then(|node| node.parent_id);
                    }
                }
                if self.has_symbol_named(*new_parent_id, &moved.kind, &moved.name, Some(*symbol_id))
                {
                    bail!(
                        "duplicate {} symbol named {} under the new parent",
                        moved.kind,
                        moved.name
                    );
                }
                let symbol = self.symbols.get_mut(symbol_id).expect("checked above");
                symbol.parent_id = *new_parent_id;
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

/// Collapse delete+create pairs that are really a move into an identity-preserving [`Operation::MoveSymbol`].
///
/// When edited source moves a symbol to a different container, a structural recover/diff sees the
/// symbol vanish from its old parent (a `DeleteSymbol`) and an identical one appear under a new
/// parent (a `CreateSymbol` with a fresh id). This pass pairs such a delete with a create that has
/// the same kind, name, and body but a *different* parent, and rewrites the pair as a single
/// `MoveSymbol` that keeps the original id — so identity survives the move. Any later operation that
/// named the discarded create id (e.g. a reference to the moved symbol) is remapped to the preserved
/// id.
///
/// This is language-agnostic: any plugin's `recover`/`diff` can post-process its output through it.
/// Conservative by design — it only collapses *leaf* symbols (no children in the base graph or in
/// the batch), and matches on identical body, so a pure move is detected while a move that also edits
/// the body is left as delete+create. A moved container with its own children is a later refinement.
pub fn detect_moves(operations: Vec<Operation>, base: &SemanticGraph) -> Vec<Operation> {
    let base_parents: BTreeSet<Uuid> = base
        .symbols
        .values()
        .filter_map(|symbol| symbol.parent_id)
        .collect();
    let batch_parents: BTreeSet<Uuid> = operations
        .iter()
        .filter_map(|op| match op {
            Operation::CreateSymbol { parent_id, .. } => *parent_id,
            _ => None,
        })
        .collect();

    let mut remap: BTreeMap<Uuid, Uuid> = BTreeMap::new();
    let mut move_at: BTreeMap<usize, (Uuid, Option<Uuid>)> = BTreeMap::new();
    let mut dropped_deletes: BTreeSet<Uuid> = BTreeSet::new();
    let mut used_create: BTreeSet<usize> = BTreeSet::new();

    for op in &operations {
        let Operation::DeleteSymbol { symbol_id } = op else {
            continue;
        };
        // Only collapse leaves whose old shape we can read from the base graph.
        if base_parents.contains(symbol_id) {
            continue;
        }
        let Some(old) = base.symbols.get(symbol_id) else {
            continue;
        };

        let matched = operations.iter().enumerate().find(|(index, candidate)| {
            if used_create.contains(index) {
                return false;
            }
            let Operation::CreateSymbol {
                symbol_id: new_id,
                parent_id,
                kind,
                name,
                body,
                ..
            } = candidate
            else {
                return false;
            };
            !batch_parents.contains(new_id)          // the created symbol is itself a leaf
                && *parent_id != old.parent_id       // genuinely re-parented
                && *kind == old.kind
                && *name == old.name
                && *body == old.body
        });

        if let Some((index, Operation::CreateSymbol { symbol_id: new_id, parent_id, .. })) = matched {
            used_create.insert(index);
            dropped_deletes.insert(*symbol_id);
            move_at.insert(index, (*symbol_id, *parent_id));
            remap.insert(*new_id, *symbol_id);
        }
    }

    if move_at.is_empty() {
        return operations;
    }

    operations
        .into_iter()
        .enumerate()
        .filter_map(|(index, op)| match op {
            Operation::DeleteSymbol { symbol_id } if dropped_deletes.contains(&symbol_id) => None,
            Operation::CreateSymbol { .. } if move_at.contains_key(&index) => {
                let (symbol_id, new_parent_id) = move_at[&index];
                Some(Operation::MoveSymbol {
                    symbol_id,
                    new_parent_id,
                })
            }
            other => Some(remap_operation(other, &remap)),
        })
        .collect()
}

/// Rewrite every symbol id in `op` through `remap` (the discarded create id → the preserved id), so
/// references and parentage that named a collapsed create now point at the surviving symbol.
fn remap_operation(op: Operation, remap: &BTreeMap<Uuid, Uuid>) -> Operation {
    fn mapped(id: Uuid, remap: &BTreeMap<Uuid, Uuid>) -> Uuid {
        remap.get(&id).copied().unwrap_or(id)
    }
    match op {
        Operation::CreateSymbol {
            symbol_id,
            parent_id,
            kind,
            name,
            body,
            metadata,
        } => Operation::CreateSymbol {
            symbol_id: mapped(symbol_id, remap),
            parent_id: parent_id.map(|id| mapped(id, remap)),
            kind,
            name,
            body,
            metadata,
        },
        Operation::DeleteSymbol { symbol_id } => Operation::DeleteSymbol {
            symbol_id: mapped(symbol_id, remap),
        },
        Operation::UpdateSymbol {
            symbol_id,
            name,
            body,
            metadata,
        } => Operation::UpdateSymbol {
            symbol_id: mapped(symbol_id, remap),
            name,
            body,
            metadata,
        },
        Operation::MoveSymbol {
            symbol_id,
            new_parent_id,
        } => Operation::MoveSymbol {
            symbol_id: mapped(symbol_id, remap),
            new_parent_id: new_parent_id.map(|id| mapped(id, remap)),
        },
        Operation::CreateReference {
            reference_id,
            from_symbol_id,
            to_symbol_id,
            kind,
        } => Operation::CreateReference {
            reference_id,
            from_symbol_id: mapped(from_symbol_id, remap),
            to_symbol_id: mapped(to_symbol_id, remap),
            kind,
        },
        Operation::DeleteReference { reference_id } => Operation::DeleteReference { reference_id },
    }
}
