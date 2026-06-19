use crate::core::{
    Branch, ChangeSet, Operation, OperationRecord, Repository, SemanticGraph, Task, metadata_string,
};
use crate::storage::{MergeResult, Storage};
use crate::ts::RenderedFile;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const DEMO_REPOSITORY: &str = "bonhomme-demo";

pub fn stable_uuid(label: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/{label}").as_bytes(),
    )
}

pub fn order_service_file_id() -> Uuid {
    stable_uuid("symbol/src/OrderService.ts")
}

pub fn order_service_class_id() -> Uuid {
    stable_uuid("symbol/OrderService")
}

pub fn display_name_method_id() -> Uuid {
    stable_uuid("symbol/OrderService/displayName")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnAgentsRequest {
    pub count: usize,
    pub include_conflicts: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoState {
    pub repository: Repository,
    pub main_branch: Branch,
    pub branches: Vec<BranchSummary>,
    pub tasks: Vec<Task>,
    pub changesets: Vec<ChangeSet>,
    pub operations: Vec<OperationView>,
    pub main_graph: SemanticGraph,
    pub rendered_files: Vec<RenderedFile>,
    pub metrics: DemoMetrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummary {
    pub id: Uuid,
    pub name: String,
    pub base_position: i64,
    pub status: BranchStatus,
    pub own_operation_count: usize,
    pub created_symbol_count: usize,
    pub created_method_names: Vec<String>,
    pub created_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BranchStatus {
    Main,
    Empty,
    Ready,
    Merged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    pub id: Uuid,
    pub branch_id: Uuid,
    pub branch_name: String,
    pub changeset_id: Uuid,
    pub position: i64,
    pub op_type: String,
    pub symbol_id: Option<Uuid>,
    pub symbol_name: Option<String>,
    pub symbol_kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoMetrics {
    pub branch_count: usize,
    pub agent_count: usize,
    pub merged_agent_count: usize,
    pub pending_agent_count: usize,
    pub operation_count: usize,
    pub symbol_count: usize,
    pub reference_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoMergeRun {
    pub results: Vec<MergeResult>,
    pub state: DemoState,
}

pub async fn reset_demo(storage: &Storage) -> Result<DemoState> {
    let (repository, main) = storage.reset_repository(DEMO_REPOSITORY).await?;
    seed_initial_order_service(storage, &repository, &main).await?;
    demo_state(storage).await
}

pub async fn ensure_demo(storage: &Storage) -> Result<DemoState> {
    match storage.repository_by_name(DEMO_REPOSITORY).await {
        Ok(_) => demo_state(storage).await,
        Err(_) => reset_demo(storage).await,
    }
}

pub async fn spawn_agents(storage: &Storage, request: SpawnAgentsRequest) -> Result<DemoState> {
    ensure_demo(storage).await?;
    let repository = storage.repository_by_name(DEMO_REPOSITORY).await?;
    let existing = storage.list_branches(repository.id).await?;
    let existing_agent_numbers = existing
        .iter()
        .filter_map(|branch| branch.name.strip_prefix("agent-"))
        .filter_map(|suffix| suffix.parse::<usize>().ok())
        .collect::<BTreeSet<_>>();
    let start = existing_agent_numbers
        .iter()
        .next_back()
        .copied()
        .unwrap_or(0)
        + 1;

    for number in start..start + request.count {
        spawn_one_agent(storage, &repository, number, request.include_conflicts).await?;
    }

    demo_state(storage).await
}

pub async fn merge_next_agent(storage: &Storage) -> Result<Option<MergeResult>> {
    let state = ensure_demo(storage).await?;
    let Some(next) = state
        .branches
        .iter()
        .filter(|branch| branch.status == BranchStatus::Ready)
        .min_by(|a, b| a.name.cmp(&b.name))
    else {
        return Ok(None);
    };

    storage
        .merge_branch(DEMO_REPOSITORY, &next.name, "main")
        .await
        .map(Some)
}

pub async fn merge_all_agents(storage: &Storage) -> Result<DemoMergeRun> {
    let mut results = Vec::new();
    let mut skipped_conflicts = BTreeSet::new();

    loop {
        let state = ensure_demo(storage).await?;
        let Some(next) = state
            .branches
            .iter()
            .filter(|branch| branch.status == BranchStatus::Ready)
            .filter(|branch| !skipped_conflicts.contains(&branch.name))
            .min_by(|a, b| a.name.cmp(&b.name))
        else {
            break;
        };
        let result = storage
            .merge_branch(DEMO_REPOSITORY, &next.name, "main")
            .await?;
        let conflicted = !result.conflicts.is_empty();
        if conflicted {
            skipped_conflicts.insert(result.source_branch.name.clone());
        }
        results.push(result);
    }

    Ok(DemoMergeRun {
        results,
        state: demo_state(storage).await?,
    })
}

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

async fn seed_initial_order_service(
    storage: &Storage,
    repository: &Repository,
    main: &Branch,
) -> Result<()> {
    let task = storage
        .create_task(repository.id, "Import TypeScript OrderService")
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            main.id,
            "Seed semantic graph from TypeScript",
            "importer",
        )
        .await?;

    storage
        .add_attachment(
            repository.id,
            "task",
            task.id,
            "PromptAttachment",
            json!({
                "model": "human",
                "prompt": "Initialize the bonhomme demo repository with a TypeScript OrderService."
            }),
        )
        .await?;

    let file_id = order_service_file_id();
    let class_id = order_service_class_id();
    let display_name_id = display_name_method_id();
    let list_orders_id = stable_uuid("symbol/OrderService/listOrders");

    let operations = vec![
        Operation::CreateSymbol {
            symbol_id: file_id,
            parent_id: None,
            kind: "file".to_string(),
            name: "OrderService.ts".to_string(),
            body: None,
            metadata: json!({"path": "src/OrderService.ts"}),
        },
        Operation::CreateSymbol {
            symbol_id: class_id,
            parent_id: Some(file_id),
            kind: "class".to_string(),
            name: "OrderService".to_string(),
            body: None,
            metadata: json!({"exported": true}),
        },
        Operation::CreateSymbol {
            symbol_id: display_name_id,
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "displayName".to_string(),
            body: Some("return \"OrderService\";".to_string()),
            metadata: json!({"signature": "displayName(): string"}),
        },
        Operation::CreateSymbol {
            symbol_id: list_orders_id,
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "listOrders".to_string(),
            body: Some("return [\"pending\", \"packed\", \"shipped\"];".to_string()),
            metadata: json!({"signature": "listOrders(): string[]"}),
        },
    ];

    for operation in operations {
        storage
            .append_operation(repository.id, main.id, changeset.id, operation)
            .await?;
    }

    Ok(())
}

async fn spawn_one_agent(
    storage: &Storage,
    repository: &Repository,
    number: usize,
    include_conflicts: bool,
) -> Result<()> {
    let agent_name = format!("agent-{number:03}");
    let branch = storage
        .create_branch(repository.id, &agent_name, "main")
        .await?;
    let task = storage
        .create_task(
            repository.id,
            &format!("{agent_name}: add an OrderService semantic method"),
        )
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            branch.id,
            &format!("{agent_name} method addition"),
            &agent_name,
        )
        .await?;
    storage
        .add_attachment(
            repository.id,
            "task",
            task.id,
            "PromptAttachment",
            json!({
                "model": format!("agent-sim-{number:03}"),
                "prompt": "Add one independent OrderService method through a semantic slice."
            }),
        )
        .await?;

    let conflict_slot = include_conflicts && number.is_multiple_of(11);
    let method_name = if conflict_slot {
        "sharedAudit".to_string()
    } else {
        format!("agent{number:03}Status")
    };
    let method_id = stable_uuid(&format!("symbol/OrderService/{agent_name}/{method_name}"));
    let reference_id = stable_uuid(&format!(
        "reference/OrderService/{agent_name}/{method_name}/displayName"
    ));
    let return_text = if conflict_slot {
        format!("return `${{this.displayName()}} shared audit from {agent_name}`;")
    } else {
        format!("return `${{this.displayName()}} accepted {agent_name}`;")
    };

    storage
        .append_operation(
            repository.id,
            branch.id,
            changeset.id,
            Operation::CreateSymbol {
                symbol_id: method_id,
                parent_id: Some(order_service_class_id()),
                kind: "method".to_string(),
                name: method_name.clone(),
                body: Some(return_text),
                metadata: json!({"signature": format!("{method_name}(): string")}),
            },
        )
        .await?;
    storage
        .append_operation(
            repository.id,
            branch.id,
            changeset.id,
            Operation::CreateReference {
                reference_id,
                from_symbol_id: method_id,
                to_symbol_id: display_name_method_id(),
                kind: "calls".to_string(),
            },
        )
        .await?;

    Ok(())
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
