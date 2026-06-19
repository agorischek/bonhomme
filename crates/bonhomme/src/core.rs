use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub name: String,
    pub base_branch_id: Option<Uuid>,
    pub base_position: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSet {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub task_id: Uuid,
    pub branch_id: Uuid,
    pub title: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub id: Uuid,
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    pub changeset_id: Uuid,
    pub position: i64,
    pub operation: Operation,
    pub created_at: DateTime<Utc>,
}

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
            | Operation::UpdateSymbol { symbol_id, .. } => {
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct SymbolNameKey {
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SymbolNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub body: Option<String>,
    pub metadata: Value,
    pub ordinal: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceNode {
    pub id: Uuid,
    pub from_symbol_id: Uuid,
    pub to_symbol_id: Uuid,
    pub kind: String,
    pub ordinal: i64,
}

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
                if let Some(parent_id) = parent_id {
                    if !self.symbols.contains_key(parent_id) {
                        bail!("parent symbol {parent_id} does not exist");
                    }
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
                if let Some(name) = name {
                    if self.has_symbol_named(
                        current.parent_id,
                        &current.kind,
                        name,
                        Some(*symbol_id),
                    ) {
                        bail!("duplicate {} symbol named {}", current.kind, name);
                    }
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
        self.validate()?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut symbol_keys = BTreeSet::new();
        for symbol in self.symbols.values() {
            if let Some(parent_id) = symbol.parent_id {
                if !self.symbols.contains_key(&parent_id) {
                    bail!("symbol {} has dangling parent {parent_id}", symbol.id);
                }
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

    pub fn children_of(&self, parent_id: Uuid) -> Vec<&SymbolNode> {
        let mut children = self
            .symbols
            .values()
            .filter(|symbol| symbol.parent_id == Some(parent_id))
            .collect::<Vec<_>>();
        children.sort_by(|a, b| {
            a.ordinal
                .cmp(&b.ordinal)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        children
    }

    pub fn root_symbols(&self) -> Vec<&SymbolNode> {
        let mut roots = self
            .symbols
            .values()
            .filter(|symbol| symbol.parent_id.is_none())
            .collect::<Vec<_>>();
        roots.sort_by(|a, b| {
            a.ordinal
                .cmp(&b.ordinal)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        roots
    }

    pub fn find_symbol(&self, name: &str) -> Vec<&SymbolNode> {
        self.symbols
            .values()
            .filter(|symbol| symbol.name == name)
            .collect()
    }

    pub fn find_references(&self, symbol_id: Uuid) -> Vec<&ReferenceNode> {
        self.references
            .values()
            .filter(|reference| {
                reference.from_symbol_id == symbol_id || reference.to_symbol_id == symbol_id
            })
            .collect()
    }

    pub fn find_callers(&self, symbol_id: Uuid) -> Vec<&SymbolNode> {
        self.references
            .values()
            .filter(|reference| reference.kind == "calls" && reference.to_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.from_symbol_id))
            .collect()
    }

    pub fn find_callees(&self, symbol_id: Uuid) -> Vec<&SymbolNode> {
        self.references
            .values()
            .filter(|reference| reference.kind == "calls" && reference.from_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.to_symbol_id))
            .collect()
    }

    pub fn find_dependencies(&self, symbol_id: Uuid) -> Vec<&SymbolNode> {
        let mut dependencies = self
            .references
            .values()
            .filter(|reference| reference.from_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.to_symbol_id))
            .collect::<Vec<_>>();
        dependencies.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        dependencies.dedup_by_key(|symbol| symbol.id);
        dependencies
    }

    pub fn find_dependents(&self, symbol_id: Uuid) -> Vec<&SymbolNode> {
        let mut dependents = self
            .references
            .values()
            .filter(|reference| reference.to_symbol_id == symbol_id)
            .filter_map(|reference| self.symbols.get(&reference.from_symbol_id))
            .collect::<Vec<_>>();
        dependents.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        dependents.dedup_by_key(|symbol| symbol.id);
        dependents
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
    Ok(graph)
}

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
            ) {
                if source_id == target_id {
                    conflicts.push(MergeConflict {
                        reason: "DUPLICATE_SYMBOL_ID".to_string(),
                        source_operation_id: source.id,
                        target_operation_id: Some(target.id),
                        symbol_id: Some(source_id),
                        detail: format!("both branches create symbol {source_id}"),
                    });
                }
            }

            if let (Some(source_id), Some(target_id)) = (
                source.operation.created_reference_id(),
                target.operation.created_reference_id(),
            ) {
                if source_id == target_id {
                    conflicts.push(MergeConflict {
                        reason: "DUPLICATE_REFERENCE_ID".to_string(),
                        source_operation_id: source.id,
                        target_operation_id: Some(target.id),
                        symbol_id: None,
                        detail: format!("both branches create reference {source_id}"),
                    });
                }
            }

            if let (Some(source_key), Some(target_key)) = (
                source.operation.created_symbol_key(),
                target.operation.created_symbol_key(),
            ) {
                if source_key == target_key {
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
            }

            let source_writes = source.operation.write_symbols();
            let target_writes = target.operation.write_symbols();
            let overlapping_symbol = source_writes.intersection(&target_writes).copied().next();

            if let Some(symbol_id) = overlapping_symbol {
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

pub fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub fn metadata_bool(metadata: &Value, key: &str) -> bool {
    metadata.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use proptest::prelude::*;
    use serde_json::json;

    fn record(position: i64, operation: Operation) -> OperationRecord {
        OperationRecord {
            id: Uuid::new_v4(),
            repository_id: Uuid::nil(),
            branch_id: Uuid::nil(),
            changeset_id: Uuid::nil(),
            position,
            operation,
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        }
    }

    fn stable_uuid(label: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("bonhomme-core-test/{label}").as_bytes(),
        )
    }

    fn stable_record(label: &str, position: i64, operation: Operation) -> OperationRecord {
        OperationRecord {
            id: stable_uuid(&format!("operation/{label}")),
            repository_id: Uuid::nil(),
            branch_id: stable_uuid(&format!("branch/{label}")),
            changeset_id: stable_uuid(&format!("changeset/{label}")),
            position,
            operation,
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        }
    }

    #[test]
    fn independent_method_additions_do_not_conflict() {
        let class_id = Uuid::new_v4();
        let source = vec![record(
            1,
            Operation::CreateSymbol {
                symbol_id: Uuid::new_v4(),
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "agentOne".to_string(),
                body: Some("return 1;".to_string()),
                metadata: json!({"signature": "agentOne(): number"}),
            },
        )];
        let target = vec![record(
            1,
            Operation::CreateSymbol {
                symbol_id: Uuid::new_v4(),
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "agentTwo".to_string(),
                body: Some("return 2;".to_string()),
                metadata: json!({"signature": "agentTwo(): number"}),
            },
        )];

        let analysis = analyze_merge(&target, &source);

        assert_eq!(analysis.outcome, MergeOutcome::SafeMerge);
        assert!(analysis.conflicts.is_empty());
    }

    #[test]
    fn duplicate_method_name_conflicts() {
        let class_id = Uuid::new_v4();
        let source = vec![record(
            1,
            Operation::CreateSymbol {
                symbol_id: Uuid::new_v4(),
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "audit".to_string(),
                body: Some("return true;".to_string()),
                metadata: json!({"signature": "audit(): boolean"}),
            },
        )];
        let target = vec![record(
            1,
            Operation::CreateSymbol {
                symbol_id: Uuid::new_v4(),
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "audit".to_string(),
                body: Some("return false;".to_string()),
                metadata: json!({"signature": "audit(): boolean"}),
            },
        )];

        let analysis = analyze_merge(&target, &source);

        assert_eq!(analysis.outcome, MergeOutcome::Conflict);
        assert_eq!(analysis.conflicts[0].reason, "DUPLICATE_SYMBOL_NAME");
    }

    #[test]
    fn independent_references_to_same_target_do_not_conflict() {
        let display_name_id = Uuid::new_v4();
        let source_method_id = Uuid::new_v4();
        let target_method_id = Uuid::new_v4();
        let source = vec![record(
            1,
            Operation::CreateReference {
                reference_id: Uuid::new_v4(),
                from_symbol_id: source_method_id,
                to_symbol_id: display_name_id,
                kind: "calls".to_string(),
            },
        )];
        let target = vec![record(
            1,
            Operation::CreateReference {
                reference_id: Uuid::new_v4(),
                from_symbol_id: target_method_id,
                to_symbol_id: display_name_id,
                kind: "calls".to_string(),
            },
        )];

        let analysis = analyze_merge(&target, &source);

        assert_eq!(analysis.outcome, MergeOutcome::SafeMerge);
    }

    #[test]
    fn dependency_queries_follow_references() {
        let file_id = Uuid::new_v4();
        let class_id = Uuid::new_v4();
        let caller_id = Uuid::new_v4();
        let callee_id = Uuid::new_v4();
        let reference_id = Uuid::new_v4();
        let records = vec![
            record(
                1,
                Operation::CreateSymbol {
                    symbol_id: file_id,
                    parent_id: None,
                    kind: "file".to_string(),
                    name: "Service.ts".to_string(),
                    body: None,
                    metadata: json!({"path": "Service.ts"}),
                },
            ),
            record(
                2,
                Operation::CreateSymbol {
                    symbol_id: class_id,
                    parent_id: Some(file_id),
                    kind: "class".to_string(),
                    name: "Service".to_string(),
                    body: None,
                    metadata: json!({}),
                },
            ),
            record(
                3,
                Operation::CreateSymbol {
                    symbol_id: caller_id,
                    parent_id: Some(class_id),
                    kind: "method".to_string(),
                    name: "caller".to_string(),
                    body: Some("return this.callee();".to_string()),
                    metadata: json!({"signature": "caller(): string"}),
                },
            ),
            record(
                4,
                Operation::CreateSymbol {
                    symbol_id: callee_id,
                    parent_id: Some(class_id),
                    kind: "method".to_string(),
                    name: "callee".to_string(),
                    body: Some("return \"ok\";".to_string()),
                    metadata: json!({"signature": "callee(): string"}),
                },
            ),
            record(
                5,
                Operation::CreateReference {
                    reference_id,
                    from_symbol_id: caller_id,
                    to_symbol_id: callee_id,
                    kind: "calls".to_string(),
                },
            ),
        ];
        let graph = materialize(&records).unwrap();

        assert_eq!(graph.find_dependencies(caller_id)[0].id, callee_id);
        assert_eq!(graph.find_dependents(callee_id)[0].id, caller_id);
    }

    proptest! {
        #[test]
        fn unique_method_additions_commute_without_conflicts(
            names in prop::collection::btree_set("[a-z][a-z0-9]{0,7}", 2..32)
        ) {
            let class_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"bonhomme-test-class");
            let names = names.into_iter().collect::<Vec<_>>();
            let midpoint = names.len() / 2;
            let make_records = |slice: &[String]| {
                slice
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        record(
                            index as i64 + 1,
                            Operation::CreateSymbol {
                                symbol_id: Uuid::new_v5(
                                    &Uuid::NAMESPACE_URL,
                                    format!("bonhomme-test-method-{name}").as_bytes(),
                                ),
                                parent_id: Some(class_id),
                                kind: "method".to_string(),
                                name: name.clone(),
                                body: Some(format!("return \"{name}\";")),
                                metadata: json!({"signature": format!("{name}(): string")}),
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let left = make_records(&names[..midpoint]);
            let right = make_records(&names[midpoint..]);

            prop_assert_eq!(analyze_merge(&left, &right).outcome, MergeOutcome::SafeMerge);
            prop_assert_eq!(analyze_merge(&right, &left).outcome, MergeOutcome::SafeMerge);
        }
    }

    #[test]
    fn simulated_large_branch_sequence_replays_deterministically() {
        let agent_count = 512;
        let first = simulate_large_branch_sequence(agent_count);
        let second = simulate_large_branch_sequence(agent_count);

        assert_eq!(first, second);
        assert_eq!(first.symbols.len(), agent_count + 3);
        assert_eq!(first.references.len(), agent_count);
        assert_eq!(first.applied_operations.len(), (agent_count * 2) + 3);
    }

    fn simulate_large_branch_sequence(agent_count: usize) -> SemanticGraph {
        let base = base_records();
        let mut graph = materialize(&base).unwrap();
        let mut target_since_base = Vec::new();
        let mut order = (0..agent_count).collect::<Vec<_>>();
        order.sort_by_key(|number| stable_order_key(&format!("agent-{number:03}")));

        for number in order {
            let source = agent_records(number);
            let analysis = analyze_merge(&target_since_base, &source);
            assert_eq!(analysis.outcome, MergeOutcome::SafeMerge);
            for record in &source {
                graph.apply_record(record).unwrap();
            }
            target_since_base.extend(source);
        }

        graph.validate().unwrap();
        graph
    }

    fn base_records() -> Vec<OperationRecord> {
        let file_id = stable_uuid("symbol/file");
        let class_id = stable_uuid("symbol/OrderService");
        let display_id = stable_uuid("symbol/OrderService/displayName");

        vec![
            stable_record(
                "base-file",
                1,
                Operation::CreateSymbol {
                    symbol_id: file_id,
                    parent_id: None,
                    kind: "file".to_string(),
                    name: "OrderService.ts".to_string(),
                    body: None,
                    metadata: json!({"path": "OrderService.ts"}),
                },
            ),
            stable_record(
                "base-class",
                2,
                Operation::CreateSymbol {
                    symbol_id: class_id,
                    parent_id: Some(file_id),
                    kind: "class".to_string(),
                    name: "OrderService".to_string(),
                    body: None,
                    metadata: json!({"exported": true}),
                },
            ),
            stable_record(
                "base-display",
                3,
                Operation::CreateSymbol {
                    symbol_id: display_id,
                    parent_id: Some(class_id),
                    kind: "method".to_string(),
                    name: "displayName".to_string(),
                    body: Some("return \"OrderService\";".to_string()),
                    metadata: json!({"signature": "displayName(): string"}),
                },
            ),
        ]
    }

    fn agent_records(number: usize) -> Vec<OperationRecord> {
        let class_id = stable_uuid("symbol/OrderService");
        let display_id = stable_uuid("symbol/OrderService/displayName");
        let method_id = stable_uuid(&format!("symbol/OrderService/agent-{number:03}"));
        let reference_id = stable_uuid(&format!("reference/agent-{number:03}/displayName"));
        let method_name = format!("agent{number:03}Status");

        vec![
            stable_record(
                &format!("agent-{number:03}-method"),
                1,
                Operation::CreateSymbol {
                    symbol_id: method_id,
                    parent_id: Some(class_id),
                    kind: "method".to_string(),
                    name: method_name.clone(),
                    body: Some(format!("return `${{this.displayName()}} {number}`;")),
                    metadata: json!({"signature": format!("{method_name}(): string")}),
                },
            ),
            stable_record(
                &format!("agent-{number:03}-reference"),
                2,
                Operation::CreateReference {
                    reference_id,
                    from_symbol_id: method_id,
                    to_symbol_id: display_id,
                    kind: "calls".to_string(),
                },
            ),
        ]
    }

    fn stable_order_key(name: &str) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}
