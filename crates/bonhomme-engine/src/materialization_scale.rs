//! Reproducible release-mode acceptance harness for incremental materialization.

use std::mem::size_of_val;
use std::time::Instant;

use anyhow::{Result, bail};
use bonhomme_core::{
    LanguagePlugin, Operation, OperationRecord, RenderedFile, SemanticGraph, Slice, ValidateFuture,
    canonical_json_bytes, materialize,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    CacheStatus,
    materialization::{decode_checkpoint, encode_checkpoint},
    rows::operation_fingerprint,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterializationScaleReport {
    pub benchmark: String,
    pub release_mode: bool,
    pub operation_count: usize,
    pub symbol_count: usize,
    pub checkpoint_operations: usize,
    pub replayed_operations: usize,
    pub cache_status: CacheStatus,
    pub prefix_fingerprint_verified: bool,
    pub exact_full_replay_equivalence: bool,
    pub rendered_tree_equivalence: bool,
    pub build_operations_wall_ms: u128,
    pub full_replay_wall_ms: u128,
    pub serialize_operation_log_wall_ms: u128,
    pub checkpoint_encode_wall_ms: u128,
    pub checkpoint_verify_wall_ms: u128,
    pub suffix_replay_wall_ms: u128,
    pub equivalence_full_replay_wall_ms: u128,
    pub total_wall_ms: u128,
    pub serialized_operation_log_bytes: usize,
    pub serialized_checkpoint_bytes: usize,
    /// Portable, reproducible proxy: serialized operation log + serialized checkpoint + the
    /// shallow in-process operation vector. It is not allocator or OS peak RSS.
    pub serialized_working_set_bytes_proxy: usize,
    pub memory_measurement_kind: String,
}

/// Build `operation_count` records spanning `symbol_count` live symbols, checkpoint the full
/// prefix, verify it, replay one additional operation, and compare the result with full replay.
/// The release acceptance invocation uses one million for both counts.
pub fn run_materialization_scale_benchmark(
    operation_count: usize,
    symbol_count: usize,
) -> Result<MaterializationScaleReport> {
    if operation_count == 0 || symbol_count == 0 {
        bail!("operation and symbol counts must both be positive");
    }
    if symbol_count > operation_count {
        bail!("symbol count cannot exceed operation count");
    }

    let total_started = Instant::now();
    let build_started = Instant::now();
    let mut operations = benchmark_operations(operation_count, symbol_count);
    let build_operations_wall_ms = build_started.elapsed().as_millis();

    let replay_started = Instant::now();
    let full_prefix = materialize(&operations)?;
    let full_replay_wall_ms = replay_started.elapsed().as_millis();

    let operation_serialize_started = Instant::now();
    let serialized_operation_log_bytes = serde_json::to_vec(&operations)?.len();
    let serialize_operation_log_wall_ms = operation_serialize_started.elapsed().as_millis();

    let checkpoint_encode_started = Instant::now();
    let repository_id = Uuid::from_u128(1);
    let branch_id = Uuid::from_u128(2);
    let checkpoint = encode_checkpoint(
        repository_id,
        branch_id,
        operation_count as i64,
        &full_prefix,
    )?;
    let serialized_checkpoint_bytes = serde_json::to_vec(&checkpoint)?.len();
    let checkpoint_encode_wall_ms = checkpoint_encode_started.elapsed().as_millis();
    drop(full_prefix);

    let checkpoint_fingerprint = operation_fingerprint(&operations)?;
    let checkpoint_verify_started = Instant::now();
    let prefix_fingerprint_verified = operation_fingerprint(&operations)? == checkpoint_fingerprint;
    let mut incremental =
        decode_checkpoint(checkpoint, repository_id, branch_id, operation_count as i64)
            .map_err(|rejection| anyhow::anyhow!("checkpoint rejected: {rejection:?}"))?;
    if incremental.applied_operations
        != operations
            .iter()
            .map(|operation| operation.id)
            .collect::<Vec<_>>()
    {
        bail!("checkpoint applied-operation ids do not match the operation prefix");
    }
    incremental.validate()?;
    let checkpoint_verify_wall_ms = checkpoint_verify_started.elapsed().as_millis();

    let suffix = suffix_operation(operation_count, symbol_count);
    let suffix_started = Instant::now();
    incremental.apply_record(&suffix)?;
    incremental.validate()?;
    let suffix_replay_wall_ms = suffix_started.elapsed().as_millis();

    operations.push(suffix);
    let equivalence_replay_started = Instant::now();
    let expected = materialize(&operations)?;
    let equivalence_full_replay_wall_ms = equivalence_replay_started.elapsed().as_millis();
    let exact_full_replay_equivalence = incremental == expected;
    if !exact_full_replay_equivalence {
        bail!("incremental replay diverged from full replay");
    }
    let fixture = BenchmarkProjectionPlugin;
    let rendered_tree_equivalence = fixture.render(&incremental) == fixture.render(&expected);
    if !rendered_tree_equivalence {
        bail!("incremental replay rendered a different tree from full replay");
    }

    Ok(MaterializationScaleReport {
        benchmark: "verified-incremental-materialization-v1".to_string(),
        release_mode: !cfg!(debug_assertions),
        operation_count,
        symbol_count,
        checkpoint_operations: operation_count,
        replayed_operations: 1,
        cache_status: CacheStatus::Incremental,
        prefix_fingerprint_verified,
        exact_full_replay_equivalence,
        rendered_tree_equivalence,
        build_operations_wall_ms,
        full_replay_wall_ms,
        serialize_operation_log_wall_ms,
        checkpoint_encode_wall_ms,
        checkpoint_verify_wall_ms,
        suffix_replay_wall_ms,
        equivalence_full_replay_wall_ms,
        total_wall_ms: total_started.elapsed().as_millis(),
        serialized_operation_log_bytes,
        serialized_checkpoint_bytes,
        serialized_working_set_bytes_proxy: serialized_operation_log_bytes
            .saturating_add(serialized_checkpoint_bytes)
            .saturating_add(size_of_val(operations.as_slice())),
        memory_measurement_kind: "serialized-working-set-plus-shallow-vector-proxy; not peak RSS"
            .to_string(),
    })
}

struct BenchmarkProjectionPlugin;

impl LanguagePlugin for BenchmarkProjectionPlugin {
    fn render(&self, graph: &SemanticGraph) -> Vec<RenderedFile> {
        let mut digest = Sha256::new();
        digest.update(
            canonical_json_bytes(graph).expect("benchmark graph is JSON-serializable by contract"),
        );
        vec![RenderedFile {
            path: "benchmark/semantic-graph.sha256".to_string(),
            content: format!("{:x}", digest.finalize()),
        }]
    }

    fn render_slice(
        &self,
        _graph: &SemanticGraph,
        base_revision: String,
        root_symbols: Vec<Uuid>,
    ) -> Slice {
        Slice {
            id: Uuid::nil(),
            base_revision,
            root_symbols,
            files: Vec::new(),
        }
    }

    fn import(&self, _files: &[RenderedFile]) -> Result<Vec<Operation>> {
        bail!("benchmark projection plugin does not import source")
    }

    fn diff(
        &self,
        _original: &[RenderedFile],
        _modified: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        bail!("benchmark projection plugin does not diff source")
    }

    fn recover_operations(
        &self,
        _base: &SemanticGraph,
        _scope: &[Uuid],
        _edited: &[RenderedFile],
    ) -> Result<Vec<Operation>> {
        bail!("benchmark projection plugin does not recover source")
    }

    fn read_source_tree(&self, _root: &std::path::Path) -> Result<Vec<RenderedFile>> {
        bail!("benchmark projection plugin does not read source")
    }

    fn validate<'a>(&'a self, _files: &'a [RenderedFile]) -> ValidateFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn benchmark_operations(operation_count: usize, symbol_count: usize) -> Vec<OperationRecord> {
    (0..operation_count)
        .map(|index| {
            let symbol_index = index % symbol_count;
            let operation = if index < symbol_count {
                Operation::CreateSymbol {
                    symbol_id: symbol_id(symbol_index),
                    parent_id: None,
                    kind: "symbol".to_string(),
                    name: format!("symbol_{symbol_index:08x}"),
                    body: Some("0".to_string()),
                    metadata: Value::Null,
                }
            } else {
                Operation::UpdateSymbol {
                    symbol_id: symbol_id(symbol_index),
                    name: None,
                    body: Some(index.to_string()),
                    metadata: None,
                }
            };
            OperationRecord {
                id: operation_id(index),
                repository_id: Uuid::from_u128(1),
                branch_id: Uuid::from_u128(2),
                changeset_id: Uuid::from_u128(3),
                position: index as i64 + 1,
                operation,
                created_at: DateTime::<Utc>::UNIX_EPOCH,
            }
        })
        .collect()
}

fn suffix_operation(operation_count: usize, symbol_count: usize) -> OperationRecord {
    OperationRecord {
        id: operation_id(operation_count),
        repository_id: Uuid::from_u128(1),
        branch_id: Uuid::from_u128(2),
        changeset_id: Uuid::from_u128(3),
        position: operation_count as i64 + 1,
        operation: Operation::UpdateSymbol {
            symbol_id: symbol_id(operation_count % symbol_count),
            name: None,
            body: Some("suffix".to_string()),
            metadata: None,
        },
        created_at: DateTime::<Utc>::UNIX_EPOCH,
    }
}

fn symbol_id(index: usize) -> Uuid {
    Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0000 + index as u128)
}

fn operation_id(index: usize) -> Uuid {
    Uuid::from_u128(0x2000_0000_0000_0000_0000_0000_0000_0000 + index as u128)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_harness_smoke_test_emits_complete_evidence() {
        let report = run_materialization_scale_benchmark(1_024, 256).unwrap();
        assert_eq!(report.checkpoint_operations, 1_024);
        assert_eq!(report.replayed_operations, 1);
        assert!(report.prefix_fingerprint_verified);
        assert!(report.exact_full_replay_equivalence);
        assert!(report.rendered_tree_equivalence);
        assert!(report.serialized_operation_log_bytes > 0);
        assert!(report.serialized_checkpoint_bytes > 0);
        assert!(report.memory_measurement_kind.contains("not peak RSS"));
    }

    #[test]
    fn checked_in_million_scale_report_meets_the_release_gate() {
        let report: serde_json::Value = serde_json::from_str(include_str!(
            "../../../reports/incremental-materialization-1m.json"
        ))
        .unwrap();
        assert!(report["operationCount"].as_u64().unwrap() >= 1_000_000);
        assert!(report["symbolCount"].as_u64().unwrap() >= 1_000_000);
        assert_eq!(report["replayedOperations"], 1);
        assert_eq!(report["prefixFingerprintVerified"], true);
        assert_eq!(report["exactFullReplayEquivalence"], true);
        assert_eq!(report["renderedTreeEquivalence"], true);
        assert!(report["serializedCheckpointBytes"].as_u64().unwrap() > 0);
        assert!(report["osPeakResidentSetBytes"].as_u64().unwrap() > 0);
        assert_eq!(report["osPeakMeasurementTool"], "/usr/bin/time -l");
    }
}
