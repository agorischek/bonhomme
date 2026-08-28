use super::{BranchStatus, BranchSummary, DEMO_REPOSITORY, DemoMetrics, DemoState, OperationView};
use anyhow::Result;
use bonhomme_core::{Branch, Operation, OperationRecord, SemanticGraph, metadata_string};
use bonhomme_engine::{ConflictKind, ConflictState, Storage};
use std::collections::BTreeMap;
use uuid::Uuid;

pub async fn demo_state(storage: &Storage) -> Result<DemoState> {
    let materialized = storage.materialize_branch(DEMO_REPOSITORY, "main").await?;
    let repository = materialized.repository.clone();
    let branches = storage.list_branches(repository.id).await?;
    let tasks = storage.list_tasks(repository.id).await?;
    let changesets = storage.list_changesets(repository.id).await?;
    let operation_records = storage.list_operations(repository.id).await?;
    let conflicts = storage.list_conflicts(repository.id).await?;
    let active_conflict_branches = conflicts
        .iter()
        .filter(|conflict| conflict.state.is_active())
        .flat_map(|conflict| [conflict.source_branch_id, conflict.target_branch_id])
        .flatten()
        .collect::<Vec<_>>();
    let resolved_merge_source_branches = conflicts
        .iter()
        .filter(|conflict| {
            conflict.kind == ConflictKind::Merge
                && conflict.state == ConflictState::Resolved
                && conflict.resolution_changeset_id.is_some()
        })
        .filter_map(|conflict| conflict.source_branch_id)
        .collect::<Vec<_>>();
    let branch_names = branches
        .iter()
        .map(|branch| (branch.id, branch.name.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut summaries = Vec::new();
    let mut base_graphs = BTreeMap::new();
    for branch in &branches {
        let has_active_conflict = active_conflict_branches.contains(&branch.id);
        let has_resolved_merge = resolved_merge_source_branches.contains(&branch.id);
        let base_branch_id = branch.base_branch_id.unwrap_or(branch.id);
        let base_key = (base_branch_id, branch.base_position);
        if let std::collections::btree_map::Entry::Vacant(entry) = base_graphs.entry(base_key) {
            let graph = storage
                .materialize_branch_at_position(base_branch_id, branch.base_position)
                .await?
                .graph;
            entry.insert(graph);
        }
        summaries.push(
            branch_summary(
                storage,
                branch,
                &base_graphs[&base_key],
                &materialized.graph,
                has_active_conflict,
                has_resolved_merge,
            )
            .await?,
        );
    }

    let operations = operation_records
        .iter()
        .map(|record| operation_view(record, &branch_names))
        .collect::<Vec<_>>();

    let agent_count = summaries
        .iter()
        .filter(|branch| branch.status != BranchStatus::Main)
        .count();
    let merged_agent_count = summaries
        .iter()
        .filter(|branch| branch.status == BranchStatus::Merged)
        .count();
    let pending_agent_count = summaries
        .iter()
        .filter(|branch| matches!(branch.status, BranchStatus::Ready | BranchStatus::Empty))
        .count();

    Ok(DemoState {
        repository,
        main_branch: materialized.branch,
        branches: summaries,
        tasks,
        changesets,
        operations,
        metrics: DemoMetrics {
            branch_count: branches.len(),
            agent_count,
            merged_agent_count,
            pending_agent_count,
            operation_count: operation_records.len(),
            symbol_count: materialized.graph.symbols.len(),
            reference_count: materialized.graph.references.len(),
        },
        main_graph: materialized.graph,
        rendered_files: materialized.files,
    })
}

async fn branch_summary(
    storage: &Storage,
    branch: &Branch,
    base_graph: &SemanticGraph,
    main_graph: &SemanticGraph,
    has_active_conflict: bool,
    has_resolved_merge: bool,
) -> Result<BranchSummary> {
    let own_operations = storage.list_own_operations(branch.id, None).await?;
    let mut operation_types = Vec::new();
    let mut touched_symbol_names = Vec::new();
    let mut created_method_names = Vec::new();
    let mut created_symbol_ids = Vec::new();

    for record in &own_operations {
        operation_types.push(record.operation.op_type().to_string());
        if let Some(name) = touched_symbol_name(&record.operation, base_graph, main_graph) {
            push_unique(&mut touched_symbol_names, name);
        }
        if let Operation::CreateSymbol {
            symbol_id,
            kind,
            name,
            ..
        } = &record.operation
        {
            created_symbol_ids.push(*symbol_id);
            if kind == "method" {
                created_method_names.push(name.clone());
            }
        }
    }

    let status = branch_status(
        branch,
        &own_operations,
        main_graph,
        has_active_conflict,
        has_resolved_merge,
    );

    Ok(BranchSummary {
        id: branch.id,
        name: branch.name.clone(),
        base_position: branch.base_position,
        status,
        own_operation_count: own_operations.len(),
        operation_types,
        touched_symbol_names,
        created_symbol_count: created_symbol_ids.len(),
        created_method_names,
        created_by: if branch.name.starts_with("agent-") {
            branch.name.clone()
        } else {
            "system".to_string()
        },
    })
}

fn branch_status(
    branch: &Branch,
    own_operations: &[OperationRecord],
    main_graph: &SemanticGraph,
    has_active_conflict: bool,
    has_resolved_merge: bool,
) -> BranchStatus {
    if branch.name == "main" {
        BranchStatus::Main
    } else if has_active_conflict {
        BranchStatus::Conflicted
    } else if has_resolved_merge {
        BranchStatus::Merged
    } else if own_operations.is_empty() {
        BranchStatus::Empty
    } else if own_operations
        .iter()
        .all(|record| operation_effect_applied(&record.operation, main_graph))
    {
        BranchStatus::Merged
    } else {
        BranchStatus::Ready
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn touched_symbol_name(
    operation: &Operation,
    base_graph: &SemanticGraph,
    main_graph: &SemanticGraph,
) -> Option<String> {
    match operation {
        Operation::CreateSymbol { kind, name, .. } if kind == "method" => Some(name.clone()),
        Operation::CreateSymbol { .. } => None,
        Operation::UpdateSymbol {
            symbol_id, name, ..
        } => name.clone().or_else(|| {
            main_graph
                .symbols
                .get(symbol_id)
                .or_else(|| base_graph.symbols.get(symbol_id))
                .map(|symbol| symbol.name.clone())
        }),
        Operation::DeleteSymbol { symbol_id } | Operation::MoveSymbol { symbol_id, .. } => {
            base_graph
                .symbols
                .get(symbol_id)
                .map(|symbol| symbol.name.clone())
        }
        Operation::CreateReference { from_symbol_id, .. } => main_graph
            .symbols
            .get(from_symbol_id)
            .or_else(|| base_graph.symbols.get(from_symbol_id))
            .map(|symbol| symbol.name.clone()),
        Operation::DeleteReference { reference_id } => base_graph
            .references
            .get(reference_id)
            .and_then(|reference| base_graph.symbols.get(&reference.from_symbol_id))
            .map(|symbol| symbol.name.clone()),
    }
}

fn operation_effect_applied(operation: &Operation, main_graph: &SemanticGraph) -> bool {
    match operation {
        Operation::CreateSymbol {
            symbol_id,
            parent_id,
            kind,
            name,
            body,
            metadata,
        } => main_graph.symbols.get(symbol_id).is_some_and(|symbol| {
            symbol.parent_id == *parent_id
                && symbol.kind == kind.as_str()
                && symbol.name == name.as_str()
                && &symbol.body == body
                && &symbol.metadata == metadata
        }),
        Operation::DeleteSymbol { symbol_id } => !main_graph.symbols.contains_key(symbol_id),
        Operation::UpdateSymbol {
            symbol_id,
            name,
            body,
            metadata,
        } => main_graph.symbols.get(symbol_id).is_some_and(|symbol| {
            name.as_ref().is_none_or(|name| symbol.name == *name)
                && body
                    .as_ref()
                    .is_none_or(|body| symbol.body.as_ref() == Some(body))
                && metadata
                    .as_ref()
                    .is_none_or(|metadata| &symbol.metadata == metadata)
        }),
        Operation::MoveSymbol {
            symbol_id,
            new_parent_id,
        } => main_graph
            .symbols
            .get(symbol_id)
            .is_some_and(|symbol| symbol.parent_id == *new_parent_id),
        Operation::CreateReference {
            reference_id,
            from_symbol_id,
            to_symbol_id,
            kind,
        } => main_graph
            .references
            .get(reference_id)
            .is_some_and(|reference| {
                reference.from_symbol_id == *from_symbol_id
                    && reference.to_symbol_id == *to_symbol_id
                    && reference.kind == kind.as_str()
            }),
        Operation::DeleteReference { reference_id } => {
            !main_graph.references.contains_key(reference_id)
        }
    }
}

fn operation_view(
    record: &OperationRecord,
    branch_names: &BTreeMap<Uuid, String>,
) -> OperationView {
    let (symbol_id, symbol_name, symbol_kind) = match &record.operation {
        Operation::CreateSymbol {
            symbol_id,
            name,
            kind,
            ..
        } => (Some(*symbol_id), Some(name.clone()), Some(kind.clone())),
        Operation::UpdateSymbol {
            symbol_id, name, ..
        } => (Some(*symbol_id), name.clone(), Some("symbol".to_string())),
        Operation::DeleteSymbol { symbol_id } => {
            (Some(*symbol_id), None, Some("symbol".to_string()))
        }
        Operation::MoveSymbol { symbol_id, .. } => {
            (Some(*symbol_id), None, Some("symbol".to_string()))
        }
        Operation::CreateReference {
            from_symbol_id,
            kind,
            ..
        } => (Some(*from_symbol_id), None, Some(kind.clone())),
        Operation::DeleteReference { reference_id } => {
            (Some(*reference_id), None, Some("reference".to_string()))
        }
    };

    OperationView {
        id: record.id,
        branch_id: record.branch_id,
        branch_name: branch_names
            .get(&record.branch_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        changeset_id: record.changeset_id,
        position: record.position,
        op_type: record.operation.op_type().to_string(),
        symbol_id,
        symbol_name: symbol_name.or_else(|| {
            if let Operation::CreateSymbol { metadata, .. } = &record.operation {
                metadata_string(metadata, "signature")
            } else {
                None
            }
        }),
        symbol_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn branch(name: &str) -> Branch {
        Branch {
            id: Uuid::new_v4(),
            repository_id: Uuid::new_v4(),
            name: name.to_string(),
            base_branch_id: None,
            base_position: 0,
            created_at: Utc::now(),
        }
    }

    fn unapplied_operation(branch_id: Uuid) -> OperationRecord {
        OperationRecord {
            id: Uuid::new_v4(),
            repository_id: Uuid::new_v4(),
            branch_id,
            changeset_id: Uuid::new_v4(),
            position: 1,
            operation: Operation::CreateSymbol {
                symbol_id: Uuid::new_v4(),
                parent_id: None,
                kind: "method".to_string(),
                name: "duplicateRiskReview".to_string(),
                body: Some("return \"risk\";".to_string()),
                metadata: json!({}),
            },
            created_at: Utc::now(),
        }
    }

    #[test]
    fn resolved_merge_conflict_marks_source_branch_landed() {
        let branch = branch("agent-022");
        let own_operations = vec![unapplied_operation(branch.id)];
        let main_graph = SemanticGraph::default();

        assert_eq!(
            branch_status(&branch, &own_operations, &main_graph, false, true),
            BranchStatus::Merged
        );
    }

    #[test]
    fn active_conflict_takes_precedence_over_resolved_merge_marker() {
        let branch = branch("agent-022");
        let own_operations = vec![unapplied_operation(branch.id)];
        let main_graph = SemanticGraph::default();

        assert_eq!(
            branch_status(&branch, &own_operations, &main_graph, true, true),
            BranchStatus::Conflicted
        );
    }
}
