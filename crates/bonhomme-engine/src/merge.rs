use crate::{MergeResult, Storage};
use anyhow::{Result, bail};
use bonhomme_core::{MergeConflict, MergeOutcome, analyze_merge, materialize};
use uuid::Uuid;

impl Storage {
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
        let target_since_base = target_operations
            .iter()
            .skip(target_base_operations.len())
            .cloned()
            .collect::<Vec<_>>();
        let source_operations = self.list_own_operations(source.id, None).await?;

        let mut analysis = analyze_merge(&target_since_base, &source_operations);
        if analysis.outcome == MergeOutcome::SafeMerge {
            let mut target_graph = materialize(&target_operations)?;
            for operation in &source_operations {
                if let Err(error) = target_graph.apply_record(operation) {
                    analysis.outcome = MergeOutcome::Conflict;
                    analysis.conflicts.push(MergeConflict {
                        reason: "VALIDATION_REJECTED".to_string(),
                        source_operation_id: operation.id,
                        target_operation_id: None,
                        symbol_id: operation.operation.created_symbol_id(),
                        detail: error.to_string(),
                    });
                    break;
                }
            }
            if analysis.outcome == MergeOutcome::SafeMerge {
                let files = self.plugin.render(&target_graph);
                if let Err(error) = self.plugin.validate(&files).await {
                    analysis.outcome = MergeOutcome::Conflict;
                    analysis.conflicts.push(MergeConflict {
                        reason: "TSC_REJECTED".to_string(),
                        source_operation_id: source_operations
                            .first()
                            .map(|operation| operation.id)
                            .unwrap_or_else(Uuid::nil),
                        target_operation_id: None,
                        symbol_id: None,
                        detail: error.to_string(),
                    });
                }
            }
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

        let mut appended = Vec::new();
        for source_operation in source_operations {
            appended.push(
                self.append_operation(
                    repository.id,
                    target.id,
                    changeset.id,
                    source_operation.operation,
                )
                .await?,
            );
        }

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
