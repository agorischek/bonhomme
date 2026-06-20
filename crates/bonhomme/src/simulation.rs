use crate::demo::{
    BranchStatus, DEMO_REPOSITORY, SpawnAgentsRequest, demo_state, reset_demo, spawn_agents,
};
use anyhow::{Result, bail};
use bonhomme_core::{Operation, RenderedFile};
use bonhomme_engine::Storage;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

const GO_DEMO_REPOSITORY: &str = "bonhomme-go-demo";
const RUST_DEMO_REPOSITORY: &str = "bonhomme-rust-demo";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationRequest {
    pub agent_count: usize,
    pub include_conflicts: bool,
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for SimulationRequest {
    fn default() -> Self {
        Self {
            agent_count: 128,
            include_conflicts: false,
            language: default_language(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationResult {
    pub repository: String,
    pub language: String,
    pub validator: String,
    pub agent_count: usize,
    pub attempted_merges: usize,
    pub safe_merges: usize,
    pub conflicts: usize,
    pub skipped_conflict_branches: Vec<String>,
    pub appended_operations: usize,
    pub final_operations: usize,
    pub final_symbols: usize,
    pub final_references: usize,
    pub rendered_files: usize,
    pub replay_deterministic: bool,
    pub render_deterministic: bool,
    pub toolchain_validated: bool,
    pub tsc_validated: bool,
}

pub async fn run_simulation(
    storage: &Storage,
    request: SimulationRequest,
) -> Result<SimulationResult> {
    if request.language == "go" {
        return run_go_simulation(storage, request).await;
    }
    if request.language == "rust" {
        return run_rust_simulation(storage, request).await;
    }
    if request.language != "typescript" {
        bail!(
            "unsupported simulation language {}; expected typescript, go, or rust",
            request.language
        );
    }

    reset_demo(storage).await?;
    spawn_agents(
        storage,
        SpawnAgentsRequest {
            count: request.agent_count,
            include_conflicts: request.include_conflicts,
        },
    )
    .await?;

    let mut attempted_merges = 0;
    let mut safe_merges = 0;
    let mut conflicts = 0;
    let mut appended_operations = 0;
    let mut skipped_conflict_branches = Vec::new();

    loop {
        let state = demo_state(storage).await?;
        let mut ready = state
            .branches
            .iter()
            .filter(|branch| branch.status == BranchStatus::Ready)
            .filter(|branch| !skipped_conflict_branches.contains(&branch.name))
            .map(|branch| branch.name.clone())
            .collect::<Vec<_>>();

        // The simulation deliberately merges in a stable *shuffled* order (FNV hash of the branch
        // name) to exercise non-sequential merge ordering, whereas the interactive demo merges
        // branches alphabetically. Both paths are independently deterministic; because ordinals are
        // assigned at append time, the two entry points can render the same agents in a different
        // sibling order. That divergence is intentional, not a determinism bug.
        ready.sort_by(|left, right| {
            stable_branch_order_key(left)
                .cmp(&stable_branch_order_key(right))
                .then_with(|| left.cmp(right))
        });

        let Some(branch_name) = ready.first() else {
            break;
        };

        attempted_merges += 1;
        let result = storage
            .merge_branch(DEMO_REPOSITORY, branch_name, "main")
            .await?;

        if result.conflicts.is_empty() {
            safe_merges += 1;
            appended_operations += result.appended_operations.len();
        } else {
            conflicts += 1;
            skipped_conflict_branches.push(result.source_branch.name);
        }
    }

    let first = storage.materialize_branch(DEMO_REPOSITORY, "main").await?;
    first.graph.validate()?;
    storage.plugin().validate(&first.files).await?;
    let second = storage.materialize_branch(DEMO_REPOSITORY, "main").await?;

    Ok(SimulationResult {
        repository: DEMO_REPOSITORY.to_string(),
        language: "typescript".to_string(),
        validator: "tsc".to_string(),
        agent_count: request.agent_count,
        attempted_merges,
        safe_merges,
        conflicts,
        skipped_conflict_branches,
        appended_operations,
        final_operations: first.operations.len(),
        final_symbols: first.graph.symbols.len(),
        final_references: first.graph.references.len(),
        rendered_files: first.files.len(),
        replay_deterministic: first.graph == second.graph,
        render_deterministic: first.files == second.files,
        toolchain_validated: true,
        tsc_validated: true,
    })
}

async fn run_go_simulation(
    storage: &Storage,
    request: SimulationRequest,
) -> Result<SimulationResult> {
    let (repository, main) = storage.reset_repository(GO_DEMO_REPOSITORY).await?;
    let task = storage
        .create_task(repository.id, "Import Go OrderService")
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            main.id,
            "Seed semantic graph from Go",
            "go-importer",
        )
        .await?;
    for operation in storage.plugin().import(&[RenderedFile {
        path: "order/service.go".to_string(),
        content: go_seed_source(),
    }])? {
        storage
            .append_operation(repository.id, main.id, changeset.id, operation)
            .await?;
    }

    let seeded = storage
        .materialize_branch(GO_DEMO_REPOSITORY, "main")
        .await?;
    let service_id = seeded.graph.find_symbol("OrderService")[0].id;
    let display_name_id = seeded.graph.find_symbol("DisplayName")[0].id;
    let list_orders_id = seeded.graph.find_symbol("ListOrders")[0].id;

    for number in 1..=request.agent_count {
        spawn_go_agent(
            storage,
            repository.id,
            service_id,
            display_name_id,
            list_orders_id,
            number,
            request.include_conflicts,
        )
        .await?;
    }

    let mut attempted_merges = 0;
    let mut safe_merges = 0;
    let mut conflicts = 0;
    let mut appended_operations = 0;
    let mut skipped_conflict_branches = Vec::new();

    loop {
        let current = storage
            .materialize_branch(GO_DEMO_REPOSITORY, "main")
            .await?;
        let branches = storage.list_branches(repository.id).await?;
        let mut ready = Vec::new();
        for branch in branches.iter().filter(|branch| {
            branch.name.starts_with("go-agent-")
                && !skipped_conflict_branches.contains(&branch.name)
        }) {
            let own_operations = storage.list_own_operations(branch.id, None).await?;
            let created_symbol_ids = own_operations
                .iter()
                .filter_map(|operation| operation.operation.created_symbol_id())
                .collect::<Vec<_>>();
            if !created_symbol_ids.is_empty()
                && created_symbol_ids
                    .iter()
                    .any(|symbol_id| !current.graph.symbols.contains_key(symbol_id))
            {
                ready.push(branch.name.clone());
            }
        }
        ready.sort_by(|left, right| {
            stable_branch_order_key(left)
                .cmp(&stable_branch_order_key(right))
                .then_with(|| left.cmp(right))
        });

        let Some(branch_name) = ready.first() else {
            break;
        };
        let branch = storage.branch_by_name(repository.id, branch_name).await?;
        let own_operations = storage.list_own_operations(branch.id, None).await?;
        if own_operations.iter().all(|operation| {
            operation
                .operation
                .created_symbol_id()
                .is_some_and(|id| seeded.graph.symbols.contains_key(&id))
        }) {
            skipped_conflict_branches.push(branch_name.clone());
            continue;
        }

        attempted_merges += 1;
        let result = storage
            .merge_branch(GO_DEMO_REPOSITORY, branch_name, "main")
            .await?;
        if result.conflicts.is_empty() {
            safe_merges += 1;
            appended_operations += result.appended_operations.len();
        } else {
            conflicts += 1;
            skipped_conflict_branches.push(result.source_branch.name);
        }
    }

    let first = storage
        .materialize_branch(GO_DEMO_REPOSITORY, "main")
        .await?;
    first.graph.validate()?;
    storage.plugin().validate(&first.files).await?;
    let second = storage
        .materialize_branch(GO_DEMO_REPOSITORY, "main")
        .await?;

    Ok(SimulationResult {
        repository: GO_DEMO_REPOSITORY.to_string(),
        language: "go".to_string(),
        validator: "go build".to_string(),
        agent_count: request.agent_count,
        attempted_merges,
        safe_merges,
        conflicts,
        skipped_conflict_branches,
        appended_operations,
        final_operations: first.operations.len(),
        final_symbols: first.graph.symbols.len(),
        final_references: first.graph.references.len(),
        rendered_files: first.files.len(),
        replay_deterministic: first.graph == second.graph,
        render_deterministic: first.files == second.files,
        toolchain_validated: true,
        tsc_validated: true,
    })
}

async fn run_rust_simulation(
    storage: &Storage,
    request: SimulationRequest,
) -> Result<SimulationResult> {
    let (repository, main) = storage.reset_repository(RUST_DEMO_REPOSITORY).await?;
    let task = storage
        .create_task(repository.id, "Import Rust OrderService")
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            main.id,
            "Seed semantic graph from Rust",
            "rust-importer",
        )
        .await?;
    for operation in storage.plugin().import(&[RenderedFile {
        path: "src/lib.rs".to_string(),
        content: rust_seed_source(),
    }])? {
        storage
            .append_operation(repository.id, main.id, changeset.id, operation)
            .await?;
    }

    let seeded = storage
        .materialize_branch(RUST_DEMO_REPOSITORY, "main")
        .await?;
    let service_id = seeded.graph.find_symbol("OrderService")[0].id;
    let display_name_id = seeded.graph.find_symbol("display_name")[0].id;
    let list_orders_id = seeded.graph.find_symbol("list_orders")[0].id;

    for number in 1..=request.agent_count {
        spawn_rust_agent(
            storage,
            repository.id,
            service_id,
            display_name_id,
            list_orders_id,
            number,
            request.include_conflicts,
        )
        .await?;
    }

    let mut attempted_merges = 0;
    let mut safe_merges = 0;
    let mut conflicts = 0;
    let mut appended_operations = 0;
    let mut skipped_conflict_branches = Vec::new();

    loop {
        let current = storage
            .materialize_branch(RUST_DEMO_REPOSITORY, "main")
            .await?;
        let branches = storage.list_branches(repository.id).await?;
        let mut ready = Vec::new();
        for branch in branches.iter().filter(|branch| {
            branch.name.starts_with("rust-agent-")
                && !skipped_conflict_branches.contains(&branch.name)
        }) {
            let own_operations = storage.list_own_operations(branch.id, None).await?;
            let created_symbol_ids = own_operations
                .iter()
                .filter_map(|operation| operation.operation.created_symbol_id())
                .collect::<Vec<_>>();
            if !created_symbol_ids.is_empty()
                && created_symbol_ids
                    .iter()
                    .any(|symbol_id| !current.graph.symbols.contains_key(symbol_id))
            {
                ready.push(branch.name.clone());
            }
        }
        ready.sort_by(|left, right| {
            stable_branch_order_key(left)
                .cmp(&stable_branch_order_key(right))
                .then_with(|| left.cmp(right))
        });

        let Some(branch_name) = ready.first() else {
            break;
        };
        let branch = storage.branch_by_name(repository.id, branch_name).await?;
        let own_operations = storage.list_own_operations(branch.id, None).await?;
        if own_operations.iter().all(|operation| {
            operation
                .operation
                .created_symbol_id()
                .is_some_and(|id| seeded.graph.symbols.contains_key(&id))
        }) {
            skipped_conflict_branches.push(branch_name.clone());
            continue;
        }

        attempted_merges += 1;
        let result = storage
            .merge_branch(RUST_DEMO_REPOSITORY, branch_name, "main")
            .await?;
        if result.conflicts.is_empty() {
            safe_merges += 1;
            appended_operations += result.appended_operations.len();
        } else {
            conflicts += 1;
            skipped_conflict_branches.push(result.source_branch.name);
        }
    }

    let first = storage
        .materialize_branch(RUST_DEMO_REPOSITORY, "main")
        .await?;
    first.graph.validate()?;
    storage.plugin().validate(&first.files).await?;
    let second = storage
        .materialize_branch(RUST_DEMO_REPOSITORY, "main")
        .await?;

    Ok(SimulationResult {
        repository: RUST_DEMO_REPOSITORY.to_string(),
        language: "rust".to_string(),
        validator: "cargo check".to_string(),
        agent_count: request.agent_count,
        attempted_merges,
        safe_merges,
        conflicts,
        skipped_conflict_branches,
        appended_operations,
        final_operations: first.operations.len(),
        final_symbols: first.graph.symbols.len(),
        final_references: first.graph.references.len(),
        rendered_files: first.files.len(),
        replay_deterministic: first.graph == second.graph,
        render_deterministic: first.files == second.files,
        toolchain_validated: true,
        tsc_validated: true,
    })
}

async fn spawn_go_agent(
    storage: &Storage,
    repository_id: Uuid,
    service_id: Uuid,
    display_name_id: Uuid,
    list_orders_id: Uuid,
    number: usize,
    include_conflicts: bool,
) -> Result<()> {
    let branch_name = format!("go-agent-{number:03}");
    let branch = storage
        .create_branch(repository_id, &branch_name, "main")
        .await?;
    let method_name = if include_conflicts && number.is_multiple_of(11) {
        "AgentDuplicateRisk".to_string()
    } else {
        format!("Agent{number:03}Status")
    };
    let symbol_id = stable_go_demo_uuid(&format!("symbol/{branch_name}/{method_name}"));
    let display_reference_id = stable_go_demo_uuid(&format!(
        "reference/{branch_name}/{method_name}/DisplayName"
    ));
    let list_reference_id =
        stable_go_demo_uuid(&format!("reference/{branch_name}/{method_name}/ListOrders"));
    let task = storage
        .create_task(repository_id, &format!("{branch_name}: add {method_name}"))
        .await?;
    let changeset = storage
        .create_changeset(
            repository_id,
            task.id,
            branch.id,
            &format!("{branch_name} {method_name}"),
            &branch_name,
        )
        .await?;
    let body = format!(
        concat!(
            "orders := s.ListOrders()\n",
            "return s.DisplayName() + \" accepted {branch_name}: \" + orders[0]"
        ),
        branch_name = branch_name
    );
    let operations = [
        Operation::CreateSymbol {
            symbol_id,
            parent_id: Some(service_id),
            kind: "method".to_string(),
            name: method_name.clone(),
            body: Some(body),
            metadata: json!({
                "signature": format!("func (s *OrderService) {method_name}() string"),
                "receiver": "OrderService",
                "path": "order/service.go",
            }),
        },
        Operation::CreateReference {
            reference_id: display_reference_id,
            from_symbol_id: symbol_id,
            to_symbol_id: display_name_id,
            kind: "calls".to_string(),
        },
        Operation::CreateReference {
            reference_id: list_reference_id,
            from_symbol_id: symbol_id,
            to_symbol_id: list_orders_id,
            kind: "calls".to_string(),
        },
    ];
    for operation in operations {
        storage
            .append_operation(repository_id, branch.id, changeset.id, operation)
            .await?;
    }
    Ok(())
}

fn go_seed_source() -> String {
    r#"
package order

type OrderService struct {
	ServiceName string
}

func (s *OrderService) DisplayName() string {
	return s.ServiceName
}

func (s *OrderService) ListOrders() []string {
	return []string{"intake", "packing", "shipped"}
}
"#
    .to_string()
}

async fn spawn_rust_agent(
    storage: &Storage,
    repository_id: Uuid,
    service_id: Uuid,
    display_name_id: Uuid,
    list_orders_id: Uuid,
    number: usize,
    include_conflicts: bool,
) -> Result<()> {
    let branch_name = format!("rust-agent-{number:03}");
    let branch = storage
        .create_branch(repository_id, &branch_name, "main")
        .await?;
    let method_name = if include_conflicts && number.is_multiple_of(11) {
        "agent_duplicate_risk".to_string()
    } else {
        format!("agent_{number:03}_status")
    };
    let symbol_id = stable_rust_demo_uuid(&format!("symbol/{branch_name}/{method_name}"));
    let display_reference_id = stable_rust_demo_uuid(&format!(
        "reference/{branch_name}/{method_name}/display_name"
    ));
    let list_reference_id = stable_rust_demo_uuid(&format!(
        "reference/{branch_name}/{method_name}/list_orders"
    ));
    let task = storage
        .create_task(repository_id, &format!("{branch_name}: add {method_name}"))
        .await?;
    let changeset = storage
        .create_changeset(
            repository_id,
            task.id,
            branch.id,
            &format!("{branch_name} {method_name}"),
            &branch_name,
        )
        .await?;
    let body = format!(
        concat!(
            "let orders = self.list_orders();\n",
            "format!(\"{{}} accepted {branch_name}: {{}}\", self.display_name(), orders[0])"
        ),
        branch_name = branch_name
    );
    let operations = [
        Operation::CreateSymbol {
            symbol_id,
            parent_id: Some(service_id),
            kind: "method".to_string(),
            name: method_name.clone(),
            body: Some(body),
            metadata: json!({
                "signature": format!("pub fn {method_name}(&self) -> String"),
                "implHeader": "impl OrderService",
                "implType": "OrderService",
                "path": "src/lib.rs",
            }),
        },
        Operation::CreateReference {
            reference_id: display_reference_id,
            from_symbol_id: symbol_id,
            to_symbol_id: display_name_id,
            kind: "calls".to_string(),
        },
        Operation::CreateReference {
            reference_id: list_reference_id,
            from_symbol_id: symbol_id,
            to_symbol_id: list_orders_id,
            kind: "calls".to_string(),
        },
    ];
    for operation in operations {
        storage
            .append_operation(repository_id, branch.id, changeset.id, operation)
            .await?;
    }
    Ok(())
}

fn rust_seed_source() -> String {
    r#"
pub struct OrderService {
    service_name: String,
}

impl OrderService {
    pub fn display_name(&self) -> &str {
        &self.service_name
    }

    pub fn list_orders(&self) -> Vec<&'static str> {
        vec!["intake", "packing", "shipped"]
    }
}
"#
    .to_string()
}

fn stable_go_demo_uuid(label: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/go-demo/{label}").as_bytes(),
    )
}

fn stable_rust_demo_uuid(label: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/rust-demo/{label}").as_bytes(),
    )
}

fn default_language() -> String {
    "typescript".to_string()
}

pub fn stable_branch_order_key(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_branch_order_key_is_repeatable() {
        let first = stable_branch_order_key("agent-042");
        let second = stable_branch_order_key("agent-042");

        assert_eq!(first, second);
        assert_ne!(first, stable_branch_order_key("agent-043"));
    }
}
