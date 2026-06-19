use crate::demo::{
    BranchStatus, DEMO_REPOSITORY, SpawnAgentsRequest, demo_state, reset_demo, spawn_agents,
};
use crate::storage::Storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationRequest {
    pub agent_count: usize,
    pub include_conflicts: bool,
}

impl Default for SimulationRequest {
    fn default() -> Self {
        Self {
            agent_count: 128,
            include_conflicts: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationResult {
    pub repository: String,
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
    pub tsc_validated: bool,
}

pub async fn run_simulation(
    storage: &Storage,
    request: SimulationRequest,
) -> Result<SimulationResult> {
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
        tsc_validated: true,
    })
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
