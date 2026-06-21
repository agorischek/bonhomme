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
                if let Some(existing) = self.symbols.get(symbol_id) {
                    bail!(
                        "duplicate symbol id {symbol_id} for {kind} symbol named {name}; \
                         existing {} symbol named {}",
                        existing.kind,
                        existing.name
                    );
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

/// Collapse delete+create runs that are really a move into identity-preserving
/// [`Operation::MoveSymbol`] (plus an [`Operation::UpdateSymbol`] when the body or metadata also
/// changed).
///
/// When edited source relocates a symbol, a structural recover/diff sees its whole subtree vanish
/// from the old parent (a run of `DeleteSymbol`) and an identical-shaped subtree appear under a new
/// parent (a run of `CreateSymbol` with fresh ids). This pass finds the moved subtree by matching it
/// structurally (kind + name, recursively) against the base graph, and rewrites it as a single
/// `MoveSymbol` of the subtree *root* that keeps the original id — descendants ride along under their
/// preserved parent, so every id in the subtree survives. Any operation that named a discarded create
/// id (e.g. a reference to the moved symbol) is remapped onto the preserved id, and a body/metadata
/// edit made during the move becomes an `UpdateSymbol`.
///
/// Language-agnostic: any plugin's `recover`/`diff` can post-process its output through it.
/// Conservative — it only collapses a subtree whose *shape* is unchanged (no children added or
/// removed during the move); a structural change keeps the safe delete+create form.
pub fn detect_moves(operations: Vec<Operation>, base: &SemanticGraph) -> Vec<Operation> {
    struct Created {
        parent: Option<Uuid>,
        kind: String,
        name: String,
        body: Option<String>,
        metadata: serde_json::Value,
    }

    // Recursive structural match: the subtree under `base_id` is identical in shape and names to the
    // subtree under `create_id` (bodies may differ — those become updates). Returns the base→create
    // id bijection for the whole subtree.
    fn match_subtree(
        base_id: Uuid,
        create_id: Uuid,
        base: &SemanticGraph,
        base_children: &BTreeMap<Uuid, Vec<Uuid>>,
        created: &BTreeMap<Uuid, Created>,
        create_children: &BTreeMap<Uuid, Vec<Uuid>>,
    ) -> Option<BTreeMap<Uuid, Uuid>> {
        let base_node = base.symbols.get(&base_id)?;
        let created_node = created.get(&create_id)?;
        if base_node.kind != created_node.kind || base_node.name != created_node.name {
            return None;
        }
        let no_children = Vec::new();
        let base_kids = base_children.get(&base_id).unwrap_or(&no_children);
        let create_kids = create_children.get(&create_id).unwrap_or(&no_children);
        if base_kids.len() != create_kids.len() {
            return None;
        }
        let mut create_by_key: BTreeMap<(String, String), Uuid> = BTreeMap::new();
        for &child in create_kids {
            let node = &created[&child];
            if create_by_key
                .insert((node.kind.clone(), node.name.clone()), child)
                .is_some()
            {
                return None; // ambiguous duplicate sibling name
            }
        }
        let mut bijection = BTreeMap::new();
        bijection.insert(base_id, create_id);
        for &child in base_kids {
            let node = base.symbols.get(&child)?;
            let create_child = *create_by_key.get(&(node.kind.clone(), node.name.clone()))?;
            let sub = match_subtree(
                child,
                create_child,
                base,
                base_children,
                created,
                create_children,
            )?;
            bijection.extend(sub);
        }
        Some(bijection)
    }

    let deleted: BTreeSet<Uuid> = operations
        .iter()
        .filter_map(|op| match op {
            Operation::DeleteSymbol { symbol_id } if base.symbols.contains_key(symbol_id) => {
                Some(*symbol_id)
            }
            _ => None,
        })
        .collect();

    let mut created: BTreeMap<Uuid, Created> = BTreeMap::new();
    let mut create_children: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for op in &operations {
        if let Operation::CreateSymbol {
            symbol_id,
            parent_id,
            kind,
            name,
            body,
            metadata,
        } = op
        {
            created.insert(
                *symbol_id,
                Created {
                    parent: *parent_id,
                    kind: kind.clone(),
                    name: name.clone(),
                    body: body.clone(),
                    metadata: metadata.clone(),
                },
            );
            if let Some(parent) = parent_id {
                create_children.entry(*parent).or_default().push(*symbol_id);
            }
        }
    }
    let mut base_children: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for symbol in base.symbols.values() {
        if let Some(parent) = symbol.parent_id {
            base_children.entry(parent).or_default().push(symbol.id);
        }
    }

    // A moved subtree's root is a deleted symbol whose parent survives (None, or a non-deleted
    // symbol); a deleted symbol whose parent is also deleted is an interior node of a larger move.
    let mut roots: Vec<Uuid> = deleted
        .iter()
        .copied()
        .filter(|id| {
            base.symbols[id]
                .parent_id
                .is_none_or(|parent| !deleted.contains(&parent))
        })
        .collect();
    roots.sort_unstable();
    let mut create_ids: Vec<Uuid> = created.keys().copied().collect();
    create_ids.sort_unstable();

    let mut consumed_deletes: BTreeSet<Uuid> = BTreeSet::new();
    let mut consumed_creates: BTreeSet<Uuid> = BTreeSet::new();
    let mut remap: BTreeMap<Uuid, Uuid> = BTreeMap::new();
    let mut root_move: BTreeMap<Uuid, (Uuid, Option<Uuid>)> = BTreeMap::new();
    let mut updates: Vec<(Uuid, Option<String>, Option<serde_json::Value>)> = Vec::new();

    for &root in &roots {
        if consumed_deletes.contains(&root) {
            continue;
        }
        let base_root = &base.symbols[&root];
        for &candidate in &create_ids {
            if consumed_creates.contains(&candidate) {
                continue;
            }
            let created_root = &created[&candidate];
            if created_root.kind != base_root.kind
                || created_root.name != base_root.name
                || created_root.parent == base_root.parent_id
            {
                continue; // not a re-parent of a matching symbol
            }
            let Some(bijection) = match_subtree(
                root,
                candidate,
                base,
                &base_children,
                &created,
                &create_children,
            ) else {
                continue;
            };
            // The whole relocated subtree must actually have been deleted — otherwise it is not a
            // clean move and we leave the operations untouched.
            if !bijection.keys().all(|id| deleted.contains(id)) {
                continue;
            }
            root_move.insert(candidate, (root, created_root.parent));
            for (&base_id, &create_id) in &bijection {
                consumed_deletes.insert(base_id);
                consumed_creates.insert(create_id);
                remap.insert(create_id, base_id);
                let base_node = &base.symbols[&base_id];
                let create_node = &created[&create_id];
                let body = (base_node.body != create_node.body)
                    .then(|| create_node.body.clone())
                    .flatten();
                let metadata = (base_node.metadata != create_node.metadata)
                    .then(|| create_node.metadata.clone());
                if body.is_some() || metadata.is_some() {
                    updates.push((base_id, body, metadata));
                }
            }
            break;
        }
    }

    if root_move.is_empty() {
        return operations;
    }

    let mut result: Vec<Operation> = operations
        .into_iter()
        .filter_map(|op| match op {
            Operation::DeleteSymbol { symbol_id } if consumed_deletes.contains(&symbol_id) => None,
            Operation::CreateSymbol { symbol_id, .. } if consumed_creates.contains(&symbol_id) => {
                root_move
                    .get(&symbol_id)
                    .map(|&(base_id, new_parent_id)| Operation::MoveSymbol {
                        symbol_id: base_id,
                        new_parent_id,
                    })
            }
            other => Some(remap_operation(other, &remap)),
        })
        .collect();

    // Body/metadata edits made during the move, applied after it (the symbols already exist).
    for (symbol_id, body, metadata) in updates {
        result.push(Operation::UpdateSymbol {
            symbol_id,
            name: None,
            body,
            metadata,
        });
    }

    result
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
