use crate::{MergeResult, Storage};
use anyhow::{Result, bail};
use bonhomme_core::{
    MergeAnalysis, MergeConflict, MergeOutcome, Operation, OperationRecord, analyze_merge,
    materialize,
};
use chrono::Utc;
use uuid::Uuid;

impl Storage {
    pub async fn analyze_operations_against_branch(
        &self,
        branch_id: Uuid,
        base_position: i64,
        operations: &[Operation],
    ) -> Result<MergeAnalysis> {
        let branch = self.branch_by_id(branch_id).await?;
        let base_operations = self
            .collect_branch_operations(branch_id, Some(base_position))
            .await?;
        if base_operations.len() as i64 != base_position {
            bail!(
                "branch {} has {} visible operations, not {}",
                branch.name,
                base_operations.len(),
                base_position
            );
        }

        let current_operations = self.collect_branch_operations(branch_id, None).await?;
        let target_since_base = current_operations
            .iter()
            .skip(base_operations.len())
            .cloned()
            .collect::<Vec<_>>();
        let source_operations =
            synthetic_operation_records(branch.repository_id, branch_id, operations);
        let mut analysis = analyze_merge(&target_since_base, &source_operations);

        if analysis.outcome == MergeOutcome::SafeMerge {
            validate_operation_application(
                self,
                &current_operations,
                &source_operations,
                &mut analysis,
            )
            .await?;
        }

        Ok(analysis)
    }

    pub async fn merge_branch(
        &self,
        repository_name: &str,
        source_branch_name: &str,
        target_branch_name: &str,
    ) -> Result<MergeResult> {
        let repository = self.repository_by_name(repository_name).await?;
        let source = self
            .branch_by_name(repository.id, source_branch_name)
            .await?;
        let target = self
            .branch_by_name(repository.id, target_branch_name)
            .await?;

        if source.base_branch_id != Some(target.id) {
            bail!(
                "bonhomme v1 merge prototype only supports direct merges from a branch based on the target branch"
            );
        }

        let target_base_operations = self
            .collect_branch_operations(target.id, Some(source.base_position))
            .await?;
        let target_operations = self.collect_branch_operations(target.id, None).await?;
        let expected_target_own_position = target_operations
            .iter()
            .filter(|operation| operation.branch_id == target.id)
            .count() as i64;
        let target_since_base = target_operations
            .iter()
            .skip(target_base_operations.len())
            .cloned()
            .collect::<Vec<_>>();
        let source_operations = self.list_own_operations(source.id, None).await?;

        let mut analysis = analyze_merge(&target_since_base, &source_operations);
        if analysis.outcome == MergeOutcome::SafeMerge {
            validate_operation_application(
                self,
                &target_operations,
                &source_operations,
                &mut analysis,
            )
            .await?;
        }

        if analysis.outcome == MergeOutcome::Conflict {
            let graph = materialize(&target_operations)?;
            return Ok(MergeResult {
                outcome: MergeOutcome::Conflict,
                conflicts: analysis.conflicts,
                source_branch: source,
                target_branch: target,
                appended_operations: Vec::new(),
                target_position: target_operations.len() as i64,
                files: self.plugin.render(&graph),
            });
        }

        let task = self
            .create_task(
                repository.id,
                &format!("Merge {source_branch_name} into {target_branch_name}"),
            )
            .await?;
        let changeset = self
            .create_changeset(
                repository.id,
                task.id,
                target.id,
                &format!("Merge {source_branch_name}"),
                "merge-engine",
            )
            .await?;

        let appended = self
            .append_operations_if_branch_position(
                repository.id,
                target.id,
                changeset.id,
                expected_target_own_position,
                source_operations
                    .iter()
                    .map(|operation| operation.operation.clone())
                    .collect(),
            )
            .await?;

        let updated_operations = self.collect_branch_operations(target.id, None).await?;
        let graph = materialize(&updated_operations)?;
        Ok(MergeResult {
            outcome: MergeOutcome::SafeMerge,
            conflicts: Vec::new(),
            source_branch: source,
            target_branch: target,
            appended_operations: appended,
            target_position: updated_operations.len() as i64,
            files: self.plugin.render(&graph),
        })
    }
}

fn synthetic_operation_records(
    repository_id: Uuid,
    branch_id: Uuid,
    operations: &[Operation],
) -> Vec<OperationRecord> {
    operations
        .iter()
        .enumerate()
        .map(|(index, operation)| OperationRecord {
            id: Uuid::new_v4(),
            repository_id,
            branch_id,
            changeset_id: Uuid::nil(),
            position: index as i64 + 1,
            operation: operation.clone(),
            created_at: Utc::now(),
        })
        .collect()
}

async fn validate_operation_application(
    storage: &Storage,
    current_operations: &[OperationRecord],
    source_operations: &[OperationRecord],
    analysis: &mut MergeAnalysis,
) -> Result<()> {
    let mut graph = materialize(current_operations)?;
    for operation in source_operations {
        if let Err(error) = graph.apply_record(operation) {
            analysis.outcome = MergeOutcome::Conflict;
            analysis.conflicts.push(MergeConflict {
                reason: "VALIDATION_REJECTED".to_string(),
                source_operation_id: operation.id,
                target_operation_id: None,
                symbol_id: operation.operation.created_symbol_id(),
                detail: error.to_string(),
            });
            return Ok(());
        }
    }

    let files = storage.plugin.render(&graph);
    if let Err(error) = storage.plugin.validate(&files).await {
        analysis.outcome = MergeOutcome::Conflict;
        analysis.conflicts.push(MergeConflict {
            reason: "COMPILER_REJECTED".to_string(),
            source_operation_id: source_operations
                .first()
                .map(|operation| operation.id)
                .unwrap_or_else(Uuid::nil),
            target_operation_id: None,
            symbol_id: None,
            detail: error.to_string(),
        });
    }
    Ok(())
}
