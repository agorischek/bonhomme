use crate::{
    CacheStatus, CheckpointRejection, MaterializationEvidence, MaterializedBranch,
    MaterializedGraph, Storage, rows::operation_fingerprint,
};
use anyhow::{Result, bail};
use bonhomme_core::{
    CURRENT_PROTOCOL_VERSION, GraphSnapshot, OperationRecord, ProtocolProvenance, RenderedFile,
    SUPPORTED_PROTOCOL_RANGE, SemanticGraph, canonical_json_bytes, materialize,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

const CHECKPOINT_FORMAT_VERSION: u16 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphCheckpointEnvelope {
    checkpoint_format: u16,
    graph_fingerprint: String,
    snapshot: GraphSnapshot,
}

struct ReplayResult {
    graph: SemanticGraph,
    cache_status: CacheStatus,
    evidence: MaterializationEvidence,
}

impl Storage {
    pub async fn materialize_branch(
        &self,
        repository_name: &str,
        branch_name: &str,
    ) -> Result<MaterializedBranch> {
        let repository = self.repository_by_name(repository_name).await?;
        let branch = self.branch_by_name(repository.id, branch_name).await?;
        let operations = self.collect_branch_operations(branch.id, None).await?;
        let replay = self
            .replay_from_latest_checkpoint(repository.id, branch.id, &operations)
            .await?;
        let files = self.plugin.render(&replay.graph);

        if replay.cache_status != CacheStatus::Hit {
            self.store_graph_cache(repository.id, branch.id, &operations, &replay.graph, &files)
                .await?;
        }

        Ok(MaterializedBranch {
            repository,
            branch,
            operations,
            graph: replay.graph,
            files,
            cache_status: replay.cache_status,
            materialization: replay.evidence,
        })
    }

    pub async fn materialize_branch_graph(
        &self,
        repository_name: &str,
        branch_name: &str,
    ) -> Result<MaterializedGraph> {
        let repository = self.repository_by_name(repository_name).await?;
        let branch = self.branch_by_name(repository.id, branch_name).await?;
        let operations = self.collect_branch_operations(branch.id, None).await?;
        let replay = self
            .replay_from_latest_checkpoint(repository.id, branch.id, &operations)
            .await?;

        if replay.cache_status != CacheStatus::Hit {
            self.store_graph_cache(repository.id, branch.id, &operations, &replay.graph, &[])
                .await?;
        }

        Ok(MaterializedGraph {
            repository,
            branch,
            operations,
            graph: replay.graph,
            cache_status: replay.cache_status,
            materialization: replay.evidence,
        })
    }

    pub async fn materialize_branch_at_position(
        &self,
        branch_id: Uuid,
        operation_count: i64,
    ) -> Result<MaterializedBranch> {
        if operation_count < 0 {
            bail!("operation position must be non-negative");
        }
        let branch = self.branch_by_id(branch_id).await?;
        let repository = self.repository_by_id(branch.repository_id).await?;
        let operations = self
            .collect_branch_operations(branch.id, Some(operation_count))
            .await?;
        require_operation_count(&branch.name, operation_count, operations.len())?;
        let replay = self
            .replay_from_latest_checkpoint(repository.id, branch.id, &operations)
            .await?;
        let files = self.plugin.render(&replay.graph);

        Ok(MaterializedBranch {
            repository,
            branch,
            operations,
            graph: replay.graph,
            files,
            cache_status: replay.cache_status,
            materialization: replay.evidence,
        })
    }

    pub async fn materialize_branch_graph_at_position(
        &self,
        branch_id: Uuid,
        operation_count: i64,
    ) -> Result<MaterializedGraph> {
        if operation_count < 0 {
            bail!("operation position must be non-negative");
        }
        let branch = self.branch_by_id(branch_id).await?;
        let repository = self.repository_by_id(branch.repository_id).await?;
        let operations = self
            .collect_branch_operations(branch.id, Some(operation_count))
            .await?;
        require_operation_count(&branch.name, operation_count, operations.len())?;
        let replay = self
            .replay_from_latest_checkpoint(repository.id, branch.id, &operations)
            .await?;

        Ok(MaterializedGraph {
            repository,
            branch,
            operations,
            graph: replay.graph,
            cache_status: replay.cache_status,
            materialization: replay.evidence,
        })
    }

    async fn replay_from_latest_checkpoint(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        operations: &[OperationRecord],
    ) -> Result<ReplayResult> {
        let total_operations = operations.len() as i64;
        let mut checkpoint_rejection = None;
        let mut prefix_fingerprint_verified = false;

        if let Some(checkpoint) = self.backend.latest_graph_checkpoint(branch_id).await? {
            let checkpoint_count = checkpoint.operation_count;
            if checkpoint_count < 0 || checkpoint_count > total_operations {
                checkpoint_rejection = Some(CheckpointRejection::AheadOfRequestedPosition);
            } else {
                let prefix = &operations[..checkpoint_count as usize];
                if operation_fingerprint(prefix)? != checkpoint.operation_fingerprint {
                    checkpoint_rejection = Some(CheckpointRejection::PrefixFingerprintMismatch);
                } else {
                    prefix_fingerprint_verified = true;
                    match decode_checkpoint(
                        checkpoint.graph,
                        repository_id,
                        branch_id,
                        checkpoint_count,
                    ) {
                        Err(rejection) => checkpoint_rejection = Some(rejection),
                        Ok(mut graph) => {
                            let expected_ids = prefix
                                .iter()
                                .map(|operation| operation.id)
                                .collect::<Vec<_>>();
                            if graph.applied_operations != expected_ids {
                                checkpoint_rejection =
                                    Some(CheckpointRejection::AppliedOperationsMismatch);
                            } else if graph.validate().is_err() {
                                checkpoint_rejection = Some(CheckpointRejection::InvalidGraph);
                            } else {
                                for operation in &operations[checkpoint_count as usize..] {
                                    graph.apply_record(operation)?;
                                }
                                graph.validate()?;
                                let replayed_operations = total_operations - checkpoint_count;
                                return Ok(ReplayResult {
                                    graph,
                                    cache_status: if replayed_operations == 0 {
                                        CacheStatus::Hit
                                    } else {
                                        CacheStatus::Incremental
                                    },
                                    evidence: MaterializationEvidence {
                                        total_operations,
                                        checkpoint_operations: checkpoint_count,
                                        replayed_operations,
                                        prefix_fingerprint_verified: true,
                                        graph_invariants_verified: true,
                                        checkpoint_rejection: None,
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }

        let graph = materialize(operations)?;
        Ok(ReplayResult {
            graph,
            cache_status: CacheStatus::Miss,
            evidence: MaterializationEvidence {
                total_operations,
                checkpoint_operations: 0,
                replayed_operations: total_operations,
                prefix_fingerprint_verified,
                graph_invariants_verified: true,
                checkpoint_rejection,
            },
        })
    }

    async fn store_graph_cache(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        operations: &[OperationRecord],
        graph: &SemanticGraph,
        files: &[RenderedFile],
    ) -> Result<()> {
        self.backend
            .store_graph_cache(
                repository_id,
                branch_id,
                operations.len() as i64,
                &operation_fingerprint(operations)?,
                encode_checkpoint(repository_id, branch_id, operations.len() as i64, graph)?,
                serde_json::to_value(files)?,
            )
            .await
    }
}

pub(crate) fn encode_checkpoint(
    repository_id: Uuid,
    branch_id: Uuid,
    position: i64,
    graph: &SemanticGraph,
) -> Result<serde_json::Value> {
    let snapshot = GraphSnapshot {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        repository_id,
        branch_id,
        position,
        graph: graph.clone(),
        provenance: ProtocolProvenance {
            actor: "system:materializer".to_string(),
            tool: format!("bonhomme-engine/{}", env!("CARGO_PKG_VERSION")),
            created_at: Utc::now(),
            source_revision: None,
            extensions: BTreeMap::new(),
        },
    };
    snapshot.validate_envelope()?;
    let envelope = GraphCheckpointEnvelope {
        checkpoint_format: CHECKPOINT_FORMAT_VERSION,
        graph_fingerprint: graph_fingerprint(graph)?,
        snapshot,
    };
    Ok(serde_json::to_value(envelope)?)
}

pub(crate) fn decode_checkpoint(
    value: serde_json::Value,
    repository_id: Uuid,
    branch_id: Uuid,
    position: i64,
) -> std::result::Result<SemanticGraph, CheckpointRejection> {
    let envelope: GraphCheckpointEnvelope =
        serde_json::from_value(value).map_err(|_| CheckpointRejection::InvalidEncoding)?;
    if envelope.checkpoint_format != CHECKPOINT_FORMAT_VERSION {
        return Err(CheckpointRejection::InvalidEncoding);
    }
    if !SUPPORTED_PROTOCOL_RANGE.contains(envelope.snapshot.protocol_version) {
        return Err(CheckpointRejection::UnsupportedProtocol);
    }
    if envelope.snapshot.repository_id != repository_id
        || envelope.snapshot.branch_id != branch_id
        || envelope.snapshot.position != position
    {
        return Err(CheckpointRejection::SnapshotContextMismatch);
    }
    envelope
        .snapshot
        .validate_context(repository_id, branch_id, position)
        .map_err(|_| CheckpointRejection::InvalidGraph)?;
    let actual = graph_fingerprint(&envelope.snapshot.graph)
        .map_err(|_| CheckpointRejection::InvalidEncoding)?;
    if actual != envelope.graph_fingerprint {
        return Err(CheckpointRejection::InvalidGraph);
    }
    Ok(envelope.snapshot.graph)
}

fn graph_fingerprint(graph: &SemanticGraph) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"bonhomme-graph-checkpoint-v1\0");
    digest.update(canonical_json_bytes(graph)?);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn require_operation_count(branch_name: &str, requested: i64, actual: usize) -> Result<()> {
    if actual as i64 != requested {
        bail!("branch {branch_name} has {actual} visible operations, not {requested}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bonhomme_core::Operation;
    use chrono::{DateTime, Utc};
    use serde_json::json;

    use super::*;

    #[test]
    fn checkpoint_graph_fingerprint_detects_valid_but_modified_graphs() {
        let mut graph = SemanticGraph::default();
        graph
            .apply_operation(
                Uuid::from_u128(1),
                &Operation::CreateSymbol {
                    symbol_id: Uuid::from_u128(2),
                    parent_id: None,
                    kind: "file".to_string(),
                    name: "a.rs".to_string(),
                    body: None,
                    metadata: json!({}),
                },
            )
            .unwrap();
        let branch_id = Uuid::from_u128(3);
        let mut value = encode_checkpoint(Uuid::from_u128(2), branch_id, 1, &graph).unwrap();
        value["snapshot"]["graph"]["symbols"][Uuid::from_u128(2).to_string()]["name"] =
            json!("changed.rs");
        assert_eq!(
            decode_checkpoint(value, Uuid::from_u128(2), branch_id, 1).unwrap_err(),
            CheckpointRejection::InvalidGraph
        );
    }

    #[test]
    fn checkpoint_requires_matching_repository_branch_position_and_major_version() {
        let repository_id = Uuid::from_u128(2);
        let branch_id = Uuid::from_u128(3);
        let graph = SemanticGraph::default();
        let value = encode_checkpoint(repository_id, branch_id, 0, &graph).unwrap();

        assert_eq!(
            decode_checkpoint(value.clone(), Uuid::from_u128(9), branch_id, 0).unwrap_err(),
            CheckpointRejection::SnapshotContextMismatch
        );
        assert_eq!(
            decode_checkpoint(value.clone(), repository_id, Uuid::from_u128(9), 0).unwrap_err(),
            CheckpointRejection::SnapshotContextMismatch
        );
        assert_eq!(
            decode_checkpoint(value.clone(), repository_id, branch_id, 1).unwrap_err(),
            CheckpointRejection::SnapshotContextMismatch
        );

        let mut incompatible = value.clone();
        incompatible["snapshot"]["protocolVersion"] = json!({"major": 2, "minor": 0});
        assert_eq!(
            decode_checkpoint(incompatible, repository_id, branch_id, 0).unwrap_err(),
            CheckpointRejection::UnsupportedProtocol
        );

        let mut additive_minor = value;
        additive_minor["snapshot"]["protocolVersion"] = json!({"major": 1, "minor": 42});
        assert_eq!(
            decode_checkpoint(additive_minor, repository_id, branch_id, 0).unwrap(),
            graph
        );
    }

    #[test]
    fn persisted_checkpoint_envelope_json_shape_is_golden() {
        let graph = SemanticGraph::default();
        let snapshot = GraphSnapshot {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            repository_id: Uuid::from_u128(2),
            branch_id: Uuid::from_u128(3),
            position: 0,
            graph: graph.clone(),
            provenance: ProtocolProvenance {
                actor: "system:materializer".to_string(),
                tool: "bonhomme-engine/test".to_string(),
                created_at: DateTime::<Utc>::UNIX_EPOCH,
                source_revision: None,
                extensions: BTreeMap::new(),
            },
        };
        let envelope = GraphCheckpointEnvelope {
            checkpoint_format: CHECKPOINT_FORMAT_VERSION,
            graph_fingerprint: graph_fingerprint(&graph).unwrap(),
            snapshot,
        };
        assert_eq!(
            serde_json::to_value(envelope).unwrap(),
            json!({
                "checkpointFormat": 1,
                "graphFingerprint": graph_fingerprint(&graph).unwrap(),
                "snapshot": {
                    "protocolVersion": {"major": 1, "minor": 0},
                    "repositoryId": "00000000-0000-0000-0000-000000000002",
                    "branchId": "00000000-0000-0000-0000-000000000003",
                    "position": 0,
                    "graph": {"symbols": {}, "references": {}, "appliedOperations": []},
                    "provenance": {
                        "actor": "system:materializer",
                        "tool": "bonhomme-engine/test",
                        "createdAt": "1970-01-01T00:00:00Z"
                    }
                }
            })
        );
    }

    #[test]
    fn full_replay_fixture_is_deterministic_for_incremental_equivalence_tests() {
        let operation = OperationRecord {
            id: Uuid::from_u128(1),
            repository_id: Uuid::from_u128(2),
            branch_id: Uuid::from_u128(3),
            changeset_id: Uuid::from_u128(4),
            position: 1,
            operation: Operation::CreateSymbol {
                symbol_id: Uuid::from_u128(5),
                parent_id: None,
                kind: "file".to_string(),
                name: "a.rs".to_string(),
                body: None,
                metadata: json!({}),
            },
            created_at: DateTime::<Utc>::UNIX_EPOCH,
        };
        assert_eq!(materialize(std::slice::from_ref(&operation)).unwrap(), {
            let mut graph = SemanticGraph::default();
            graph.apply_record(&operation).unwrap();
            graph
        });
    }
}
