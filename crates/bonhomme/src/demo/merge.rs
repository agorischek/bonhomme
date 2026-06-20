use super::{BranchStatus, DEMO_REPOSITORY, DemoMergeRun, demo_state, ensure_demo};
use anyhow::Result;
use bonhomme_engine::{MergeResult, Storage};
use std::collections::BTreeSet;

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
