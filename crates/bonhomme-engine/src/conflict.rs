use crate::{
    ConflictKind, ConflictPositions, ConflictRecord, ConflictState, MergeResult,
    ResolveConflictResult, StillConflictingReason, Storage,
    backend::{
        BranchAdvanced, PendingAttachment, PendingConflictResolutionCommit, PendingOperation,
        PendingOperationCommit,
    },
    models::PendingConflict,
};
use anyhow::{Context, Result, bail};
use bonhomme_core::{
    Branch, MergeConflict, MergeOutcome, Operation, OperationRecord, RenderedFile, SemanticGraph,
    analyze_merge, detect_moves, materialize, metadata_string,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictViews {
    pub conflict: ConflictRecord,
    pub base_files: Vec<RenderedFile>,
    pub current_files: Vec<RenderedFile>,
    pub proposed_files: Vec<RenderedFile>,
    pub resolved_files: Vec<RenderedFile>,
}

impl Storage {
    pub async fn list_conflicts(&self, repository_id: Uuid) -> Result<Vec<ConflictRecord>> {
        self.backend.list_conflicts(repository_id).await
    }

    pub async fn list_conflicts_by_repository_name(
        &self,
        repository_name: &str,
    ) -> Result<Vec<ConflictRecord>> {
        let repository = self.repository_by_name(repository_name).await?;
        self.list_conflicts(repository.id).await
    }

    pub async fn conflict_by_id(&self, conflict_id: Uuid) -> Result<ConflictRecord> {
        self.backend.conflict_by_id(conflict_id).await
    }

    pub async fn abandon_conflict(&self, conflict_id: Uuid) -> Result<ConflictRecord> {
        self.backend
            .update_conflict_state(conflict_id, ConflictState::Abandoned)
            .await
    }

    pub async fn mark_conflict_resolving(&self, conflict_id: Uuid) -> Result<ConflictRecord> {
        self.backend
            .update_conflict_state(conflict_id, ConflictState::Resolving)
            .await
    }

    pub async fn create_slice_apply_conflict(
        &self,
        slice_id: Uuid,
        branch: &Branch,
        positions: ConflictPositions,
        conflicts: Vec<MergeConflict>,
        proposed_operations: Vec<Operation>,
        created_by: &str,
    ) -> Result<ConflictRecord> {
        self.create_conflict(PendingConflict {
            repository_id: branch.repository_id,
            kind: ConflictKind::SliceApply,
            source_branch_id: branch.id,
            target_branch_id: branch.id,
            source_branch_name: branch.name.clone(),
            target_branch_name: branch.name.clone(),
            slice_id: Some(slice_id),
            source_changeset_id: None,
            base_position: positions.base,
            target_position: positions.target,
            conflicts,
            proposed_operations,
            created_by: created_by.to_string(),
        })
        .await
    }

    pub(crate) async fn create_merge_conflict(
        &self,
        source: &Branch,
        target: &Branch,
        positions: ConflictPositions,
        conflicts: Vec<MergeConflict>,
        source_operations: &[OperationRecord],
        created_by: &str,
    ) -> Result<ConflictRecord> {
        self.create_conflict(PendingConflict {
            repository_id: target.repository_id,
            kind: ConflictKind::Merge,
            source_branch_id: source.id,
            target_branch_id: target.id,
            source_branch_name: source.name.clone(),
            target_branch_name: target.name.clone(),
            slice_id: None,
            source_changeset_id: source_operations
                .first()
                .map(|operation| operation.changeset_id),
            base_position: positions.base,
            target_position: positions.target,
            conflicts,
            proposed_operations: source_operations
                .iter()
                .map(|operation| operation.operation.clone())
                .collect(),
            created_by: created_by.to_string(),
        })
        .await
    }

    async fn create_conflict(&self, conflict: PendingConflict) -> Result<ConflictRecord> {
        let attachment = PendingAttachment {
            attachment_type: "ConflictAttachment".to_string(),
            payload: json!({
                "kind": conflict.kind.clone(),
                "state": ConflictState::Open,
                "sourceBranch": conflict.source_branch_name.clone(),
                "targetBranch": conflict.target_branch_name.clone(),
                "basePosition": conflict.base_position,
                "targetPosition": conflict.target_position,
                "conflictCount": conflict.conflicts.len(),
                "proposedOperationCount": conflict.proposed_operations.len(),
            }),
        };
        self.backend
            .insert_conflict_with_attachment(conflict, attachment)
            .await
    }

    pub async fn conflict_views(&self, conflict_id: Uuid) -> Result<ConflictViews> {
        let conflict = self.conflict_by_id(conflict_id).await?;
        let target_branch_id = conflict
            .target_branch_id
            .context("conflict target branch has been deleted")?;

        let base = self
            .materialize_branch_at_position(target_branch_id, conflict.base_position)
            .await?;
        let current_operations = self
            .collect_branch_operations(target_branch_id, None)
            .await?;
        let current_graph = materialize(&current_operations)?;
        let current_files = self.plugin.render(&current_graph);

        let mut proposed_graph = base.graph.clone();
        for operation in synthetic_operation_records(
            conflict.repository_id,
            target_branch_id,
            &conflict.proposed_operations,
        ) {
            proposed_graph.apply_record(&operation)?;
        }
        let proposed_files = self.plugin.render(&proposed_graph);

        Ok(ConflictViews {
            conflict,
            base_files: base.files,
            resolved_files: current_files.clone(),
            current_files,
            proposed_files,
        })
    }

    pub async fn resolve_conflict(
        &self,
        conflict_id: Uuid,
        resolved_files: Vec<RenderedFile>,
        agent: &str,
        accept_drop: bool,
    ) -> Result<ResolveConflictResult> {
        let conflict = self.conflict_by_id(conflict_id).await?;
        if !conflict.state.is_active() {
            return Ok(still_conflicting(
                conflict,
                StillConflictingReason::ConflictNotOpen,
                "conflict is not open or resolving",
                Vec::new(),
                Vec::new(),
            ));
        }
        let Some(target_branch_id) = conflict.target_branch_id else {
            return Ok(still_conflicting(
                conflict,
                StillConflictingReason::MissingTargetBranch,
                "conflict target branch has been deleted",
                Vec::new(),
                Vec::new(),
            ));
        };
        let target_branch = self.branch_by_id(target_branch_id).await?;
        let current_operations = self
            .collect_branch_operations(target_branch_id, None)
            .await?;
        let expected_target_own_position = self
            .list_own_operations(target_branch_id, None)
            .await?
            .len() as i64;
        let current_graph = materialize(&current_operations)?;
        let current_files = self.plugin.render(&current_graph);

        let recovered = match self.recover_resolution_operations(
            &current_graph,
            &current_files,
            &resolved_files,
        ) {
            Ok(operations) => operations,
            Err(error) => {
                return Ok(still_conflicting(
                    conflict,
                    StillConflictingReason::RecoveryRejected,
                    format!("failed to recover resolved files: {error:#}"),
                    Vec::new(),
                    Vec::new(),
                ));
            }
        };
        let recovered = detect_moves(recovered, &current_graph);

        let (faithful, unfaithful_files) =
            self.recovery_is_faithful(&current_graph, &recovered, &resolved_files);
        if !faithful {
            return Ok(still_conflicting(
                conflict,
                StillConflictingReason::UnfaithfulRecovery,
                "recovered operations did not render back to the resolved files",
                Vec::new(),
                unfaithful_files,
            ));
        }

        if recovered.is_empty() && !conflict.proposed_operations.is_empty() && !accept_drop {
            return Ok(still_conflicting(
                conflict,
                StillConflictingReason::RequiresDropAcknowledgement,
                "resolution carries none of the proposed operations; pass accept_drop to keep current and drop source intent",
                Vec::new(),
                Vec::new(),
            ));
        }

        let source_records =
            synthetic_operation_records(conflict.repository_id, target_branch_id, &recovered);
        let mut analysis = analyze_merge(&[], &source_records);
        if analysis.outcome == MergeOutcome::SafeMerge {
            validate_operation_application(
                self,
                &current_operations,
                &source_records,
                &mut analysis,
            )
            .await?;
        }
        if analysis.outcome == MergeOutcome::Conflict {
            return Ok(still_conflicting(
                conflict,
                StillConflictingReason::ValidationRejected,
                "resolution operations conflict with the current target graph",
                analysis.conflicts.clone(),
                Vec::new(),
            ));
        }

        let rendered_after_resolution =
            render_after_operations(&current_graph, &source_records, self)?;
        if let Err(error) = self.plugin.validate(&rendered_after_resolution).await {
            return Ok(still_conflicting(
                conflict,
                StillConflictingReason::CompilerRejected,
                format!("toolchain rejected resolved files: {error:#}"),
                Vec::new(),
                Vec::new(),
            ));
        }

        let resolution = PendingConflictResolutionCommit {
            commit: PendingOperationCommit {
                repository_id: conflict.repository_id,
                branch_id: target_branch.id,
                expected_current_position: expected_target_own_position,
                task_title: format!("Resolve conflict {}", conflict.id),
                changeset_title: format!("Resolve conflict {}", conflict.id),
                created_by: agent.to_string(),
                operations: recovered
                    .iter()
                    .map(|operation| {
                        Ok(PendingOperation {
                            op_type: operation.op_type().to_string(),
                            payload: serde_json::to_value(operation)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            },
            conflict_id: conflict.id,
            attachment: PendingAttachment {
                attachment_type: "ConflictResolutionAttachment".to_string(),
                payload: json!({
                    "conflictId": conflict.id,
                    "resolvedFiles": resolved_files.iter().map(|file| &file.path).collect::<Vec<_>>(),
                    "recoveredOperationCount": recovered.len(),
                    "acceptedDrop": accept_drop,
                }),
            },
            resolved_by: agent.to_string(),
            acknowledged_drop: accept_drop,
        };
        let committed = match self.backend.commit_conflict_resolution(resolution).await {
            Ok(committed) => committed,
            Err(error) => {
                if error.downcast_ref::<BranchAdvanced>().is_none() {
                    return Err(error);
                }
                return Ok(still_conflicting(
                    conflict,
                    StillConflictingReason::BranchAdvanced,
                    format!("target branch advanced while resolving: {error:#}"),
                    Vec::new(),
                    Vec::new(),
                ));
            }
        };

        Ok(ResolveConflictResult::Resolved {
            conflict: Box::new(committed.conflict),
            changeset_id: committed.changeset.id,
            appended_operations: committed.operations,
            resumed_merge: None::<Box<MergeResult>>,
        })
    }

    fn recover_resolution_operations(
        &self,
        current_graph: &SemanticGraph,
        current_files: &[RenderedFile],
        resolved_files: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        reject_duplicate_paths(current_files)?;
        reject_duplicate_paths(resolved_files)?;
        let current_by_path = files_by_path(current_files);
        let resolved_by_path = files_by_path(resolved_files);
        let file_symbols = file_symbols_by_path(current_graph);

        let mut deleted_paths = Vec::new();
        let mut changed_scope = Vec::new();
        let mut changed_files = Vec::new();
        let mut added_files = Vec::new();

        for file in current_files {
            match resolved_by_path.get(file.path.as_str()) {
                Some(resolved) if resolved.content == file.content => {}
                Some(resolved) => {
                    let symbol = file_symbols
                        .get(file.path.as_str())
                        .with_context(|| format!("no file symbol found for {}", file.path))?;
                    changed_scope.push(symbol.id);
                    changed_files.push((*resolved).clone());
                }
                None => deleted_paths.push(file.path.as_str()),
            }
        }
        for file in resolved_files {
            if !current_by_path.contains_key(file.path.as_str()) {
                added_files.push(file.clone());
            }
        }

        let mut operations = delete_file_operations(current_graph, deleted_paths);
        if !changed_scope.is_empty() {
            operations.extend(self.plugin.recover_operations(
                current_graph,
                &changed_scope,
                &changed_files,
            )?);
        }
        if !added_files.is_empty() {
            operations.extend(self.plugin.import(&added_files)?);
        }
        Ok(operations)
    }

    fn recovery_is_faithful(
        &self,
        current_graph: &SemanticGraph,
        operations: &[Operation],
        resolved_files: &[RenderedFile],
    ) -> (bool, Vec<String>) {
        let records = synthetic_operation_records(Uuid::nil(), Uuid::nil(), operations);
        match render_after_operations(current_graph, &records, self) {
            Ok(rendered) => compare_rendered_files(&rendered, resolved_files),
            Err(error) => (false, vec![format!("operation replay failed: {error:#}")]),
        }
    }
}

fn still_conflicting(
    conflict: ConflictRecord,
    reason: StillConflictingReason,
    diagnostic: impl Into<String>,
    conflicts: Vec<MergeConflict>,
    unfaithful_files: Vec<String>,
) -> ResolveConflictResult {
    ResolveConflictResult::StillConflicting {
        conflict: Box::new(conflict),
        reason,
        conflicts,
        unfaithful_files,
        diagnostic: diagnostic.into(),
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
    analysis: &mut bonhomme_core::MergeAnalysis,
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

fn render_after_operations(
    current_graph: &SemanticGraph,
    source_operations: &[OperationRecord],
    storage: &Storage,
) -> Result<Vec<RenderedFile>> {
    let mut graph = current_graph.clone();
    for operation in source_operations {
        graph.apply_record(operation)?;
    }
    graph.validate()?;
    Ok(storage.plugin.render(&graph))
}

#[derive(Clone)]
struct FileSymbolInfo {
    id: Uuid,
}

fn file_symbols_by_path(graph: &SemanticGraph) -> BTreeMap<String, FileSymbolInfo> {
    graph
        .root_symbols()
        .into_iter()
        .filter(|symbol| symbol.kind == "file")
        .map(|symbol| {
            let path =
                metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone());
            (path, FileSymbolInfo { id: symbol.id })
        })
        .collect()
}

fn delete_file_operations<'a>(
    graph: &SemanticGraph,
    paths: impl IntoIterator<Item = &'a str>,
) -> Vec<Operation> {
    let file_symbols = file_symbols_by_path(graph);
    let mut delete_ids = BTreeSet::new();
    let mut symbol_deletes = Vec::new();
    for path in paths {
        if let Some(file) = file_symbols.get(path) {
            collect_delete_symbols(graph, file.id, &mut delete_ids, &mut symbol_deletes);
        }
    }
    let mut reference_deletes = graph
        .references
        .values()
        .filter(|reference| {
            delete_ids.contains(&reference.from_symbol_id)
                || delete_ids.contains(&reference.to_symbol_id)
        })
        .map(|reference| (reference.ordinal, reference.id))
        .collect::<Vec<_>>();
    reference_deletes.sort();

    reference_deletes
        .into_iter()
        .map(|(_, reference_id)| Operation::DeleteReference { reference_id })
        .chain(
            symbol_deletes
                .into_iter()
                .map(|symbol_id| Operation::DeleteSymbol { symbol_id }),
        )
        .collect()
}

fn collect_delete_symbols(
    graph: &SemanticGraph,
    symbol_id: Uuid,
    delete_ids: &mut BTreeSet<Uuid>,
    symbol_deletes: &mut Vec<Uuid>,
) {
    if !delete_ids.insert(symbol_id) {
        return;
    }
    for child in graph.children_of(symbol_id) {
        collect_delete_symbols(graph, child.id, delete_ids, symbol_deletes);
    }
    symbol_deletes.push(symbol_id);
}

fn files_by_path(files: &[RenderedFile]) -> BTreeMap<&str, &RenderedFile> {
    files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect()
}

fn reject_duplicate_paths(files: &[RenderedFile]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for file in files {
        if !seen.insert(file.path.as_str()) {
            bail!("duplicate resolved file path {}", file.path);
        }
    }
    Ok(())
}

fn compare_rendered_files(
    rendered: &[RenderedFile],
    expected: &[RenderedFile],
) -> (bool, Vec<String>) {
    let rendered_by_path = files_by_path(rendered);
    let expected_by_path = files_by_path(expected);
    let mut paths = BTreeSet::new();
    paths.extend(rendered_by_path.keys().copied());
    paths.extend(expected_by_path.keys().copied());

    let mut unfaithful = Vec::new();
    for path in paths {
        match (rendered_by_path.get(path), expected_by_path.get(path)) {
            (Some(rendered), Some(expected))
                if normalize_source(&rendered.content) == normalize_source(&expected.content) => {}
            (Some(_), Some(_)) => unfaithful.push(path.to_string()),
            (Some(_), None) => {
                unfaithful.push(format!("{path} was rendered but absent from resolved"))
            }
            (None, Some(_)) => {
                unfaithful.push(format!("{path} was present in resolved but not rendered"))
            }
            (None, None) => {}
        }
    }
    (unfaithful.is_empty(), unfaithful)
}

fn normalize_source(source: &str) -> String {
    source.replace("\r\n", "\n").trim_end().to_string()
}
