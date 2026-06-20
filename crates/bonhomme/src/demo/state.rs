use super::{BranchStatus, BranchSummary, DEMO_REPOSITORY, DemoMetrics, DemoState, OperationView};
use anyhow::Result;
use bonhomme_core::{Branch, Operation, OperationRecord, SemanticGraph, metadata_string};
use bonhomme_engine::Storage;
use std::collections::BTreeMap;
use uuid::Uuid;

pub async fn demo_state(storage: &Storage) -> Result<DemoState> {
    let materialized = storage.materialize_branch(DEMO_REPOSITORY, "main").await?;
    let repository = materialized.repository.clone();
    let branches = storage.list_branches(repository.id).await?;
    let tasks = storage.list_tasks(repository.id).await?;
    let changesets = storage.list_changesets(repository.id).await?;
    let operation_records = storage.list_operations(repository.id).await?;
    let branch_names = branches
        .iter()
        .map(|branch| (branch.id, branch.name.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut summaries = Vec::new();
    for branch in &branches {
        summaries.push(branch_summary(storage, branch, &materialized.graph).await?);
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
    main_graph: &SemanticGraph,
) -> Result<BranchSummary> {
    let own_operations = storage.list_own_operations(branch.id, None).await?;
    let mut created_method_names = Vec::new();
    let mut created_symbol_ids = Vec::new();

    for record in &own_operations {
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

    let status = if branch.name == "main" {
        BranchStatus::Main
    } else if own_operations.is_empty() {
        BranchStatus::Empty
    } else if !created_symbol_ids.is_empty()
        && created_symbol_ids
            .iter()
            .all(|symbol_id| main_graph.symbols.contains_key(symbol_id))
    {
        BranchStatus::Merged
    } else {
        BranchStatus::Ready
    };

    Ok(BranchSummary {
        id: branch.id,
        name: branch.name.clone(),
        base_position: branch.base_position,
        status,
        own_operation_count: own_operations.len(),
        created_symbol_count: created_symbol_ids.len(),
        created_method_names,
        created_by: if branch.name.starts_with("agent-") {
            branch.name.clone()
        } else {
            "system".to_string()
        },
    })
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
