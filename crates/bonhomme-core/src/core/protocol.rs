//! Stable wire contracts for exchanging semantic operations and graph snapshots.
//!
//! The storage model predates this envelope and continues to deserialize bare [`Operation`] values.
//! New interchange points should use [`OperationBatch`] or [`GraphSnapshot`] so a reader can reject
//! incompatible data before replaying it. Version 1 follows additive-minor/strict-major semantics:
//! a reader accepts only versions in its advertised [`ProtocolRange`], unknown fields are ignored by
//! serde, and changing the meaning or serialized spelling of an existing field requires a new major
//! version.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{Operation, SemanticGraph};

/// The current semantic interchange protocol. This is deliberately independent of crate/package
/// versions: implementations may release without changing persisted semantics.
pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0);

/// The versions this implementation can read and write without a migration.
pub const SUPPORTED_PROTOCOL_RANGE: ProtocolRange = ProtocolRange {
    minimum: ProtocolVersion::new(1, 0),
    // V1 minor releases are additive. Serde ignores unknown fields, so a V1 reader can accept any
    // later V1 minor while writers continue to emit CURRENT_PROTOCOL_VERSION.
    maximum: ProtocolVersion::new(1, u16::MAX),
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolRange {
    pub minimum: ProtocolVersion,
    pub maximum: ProtocolVersion,
}

impl ProtocolRange {
    pub const fn current() -> Self {
        SUPPORTED_PROTOCOL_RANGE
    }

    pub const fn contains(&self, version: ProtocolVersion) -> bool {
        version.major == self.minimum.major
            && version.major == self.maximum.major
            && version.minor >= self.minimum.minor
            && version.minor <= self.maximum.minor
    }

    pub fn require_supported(&self, version: ProtocolVersion) -> Result<()> {
        if !self.contains(version) {
            bail!(
                "semantic protocol {version} is not supported; this reader accepts {} through {}",
                self.minimum,
                self.maximum
            );
        }
        Ok(())
    }
}

/// Immutable origin data for an operation batch or graph snapshot. Extensions are namespaced by
/// convention (for example `"example.com/buildId"`) and are included in canonical signing bytes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolProvenance {
    pub actor: String,
    pub tool: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

/// A detached signature over [`OperationBatch::signing_payload`]. The core intentionally does not
/// prescribe key storage or a cryptographic provider; the algorithm name, key id, and encoded value
/// travel with the immutable batch while verification remains a deployment policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DetachedSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}

/// A replayable, versioned sequence of semantic operations.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationBatch {
    pub protocol_version: ProtocolVersion,
    pub batch_id: Uuid,
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    /// The authoritative operation position immediately before this batch. This makes stale writes
    /// detectable without interpreting a branch name or Git ref.
    pub base_position: i64,
    pub operations: Vec<Operation>,
    pub provenance: ProtocolProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<DetachedSignature>,
}

impl OperationBatch {
    pub fn new(
        batch_id: Uuid,
        repository_id: Uuid,
        branch_id: Uuid,
        base_position: i64,
        operations: Vec<Operation>,
        provenance: ProtocolProvenance,
    ) -> Self {
        Self {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            batch_id,
            repository_id,
            branch_id,
            base_position,
            operations,
            provenance,
            signature: None,
        }
    }

    pub fn validate_envelope(&self) -> Result<()> {
        SUPPORTED_PROTOCOL_RANGE.require_supported(self.protocol_version)?;
        if self.base_position < 0 {
            bail!("operation batch basePosition must be non-negative");
        }
        validate_provenance(&self.provenance, "operation batch")?;
        if let Some(signature) = &self.signature
            && (signature.algorithm.trim().is_empty()
                || signature.key_id.trim().is_empty()
                || signature.value.trim().is_empty())
        {
            bail!("detached signatures require algorithm, keyId, and value");
        }
        Ok(())
    }

    /// Validate both the wire envelope and the storage/export context supplied by the caller.
    /// Context is never inferred from the payload: a valid batch for another repository or branch
    /// must not be accepted merely because its operations deserialize.
    pub fn validate_context(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        base_position: i64,
    ) -> Result<()> {
        self.validate_envelope()?;
        if self.repository_id != repository_id
            || self.branch_id != branch_id
            || self.base_position != base_position
        {
            bail!(
                "operation batch repository, branch, or base position does not match its context"
            );
        }
        Ok(())
    }

    /// Canonical UTF-8 JSON bytes with the signature omitted. Signers and verifiers must use these
    /// exact bytes rather than reserializing the batch independently.
    pub fn signing_payload(&self) -> Result<Vec<u8>> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        canonical_json_bytes(&unsigned)
    }
}

/// A versioned materialization checkpoint. `graph.validate()` is part of envelope validation so a
/// corrupt or incompatible snapshot never becomes a replay base.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    pub protocol_version: ProtocolVersion,
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    pub position: i64,
    pub graph: SemanticGraph,
    pub provenance: ProtocolProvenance,
}

impl GraphSnapshot {
    pub fn validate_envelope(&self) -> Result<()> {
        SUPPORTED_PROTOCOL_RANGE.require_supported(self.protocol_version)?;
        if self.position < 0 {
            bail!("graph snapshot position must be non-negative");
        }
        validate_provenance(&self.provenance, "graph snapshot")?;
        self.graph.validate()
    }

    /// Validate the envelope against the authoritative row/key used to retrieve it.
    pub fn validate_context(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        position: i64,
    ) -> Result<()> {
        self.validate_envelope()?;
        if self.repository_id != repository_id
            || self.branch_id != branch_id
            || self.position != position
        {
            bail!("graph snapshot repository, branch, or position does not match its context");
        }
        Ok(())
    }
}

fn validate_provenance(provenance: &ProtocolProvenance, label: &str) -> Result<()> {
    if provenance.actor.trim().is_empty() || provenance.tool.trim().is_empty() {
        bail!("{label} provenance requires non-empty actor and tool values");
    }
    Ok(())
}

/// Context required to upgrade an old, unversioned `Vec<Operation>` payload into protocol v1.
/// Migration is explicit because none of these identity/provenance values can be inferred safely.
#[derive(Clone, Debug)]
pub struct LegacyBatchContext {
    pub batch_id: Uuid,
    pub repository_id: Uuid,
    pub branch_id: Uuid,
    pub base_position: i64,
    pub provenance: ProtocolProvenance,
}

pub fn migrate_legacy_operations(
    operations: Vec<Operation>,
    context: LegacyBatchContext,
) -> OperationBatch {
    OperationBatch::new(
        context.batch_id,
        context.repository_id,
        context.branch_id,
        context.base_position,
        operations,
        context.provenance,
    )
}

/// A stable input for UUIDv5 symbol identities in new plugins. V1 identities are deliberately
/// path-sensitive: graph-anchored recovery preserves an existing id across moves, while a fresh
/// import of a relocated file receives a new identity. That limitation is visible in plugin
/// capability reports rather than hidden behind a stronger claim.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SymbolIdentityV1 {
    pub handler: String,
    pub repository_relative_path: String,
    pub kind: String,
    pub qualified_name: String,
}

impl SymbolIdentityV1 {
    pub fn stable_id(&self) -> Result<Uuid> {
        super::super::lang::safe_relative_path(&self.repository_relative_path)?;
        for (label, value) in [
            ("handler", &self.handler),
            ("kind", &self.kind),
            ("qualifiedName", &self.qualified_name),
        ] {
            if value.trim().is_empty() {
                bail!("semantic identity {label} must not be empty");
            }
        }

        let canonical = canonical_json_bytes(self)?;
        let mut namespaced = b"https://bonhomme.dev/semantic-identity/v1\0".to_vec();
        namespaced.extend(canonical);
        Ok(Uuid::new_v5(&Uuid::NAMESPACE_URL, &namespaced))
    }
}

/// Serialize JSON deterministically: object keys are recursively sorted, arrays retain order, and
/// serde_json's compact UTF-8 spelling is used for strings and numbers.
pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    fn sort_value(value: Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted = map
                    .into_iter()
                    .map(|(key, value)| (key, sort_value(value)))
                    .collect::<BTreeMap<_, _>>();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(values) => Value::Array(values.into_iter().map(sort_value).collect()),
            other => other,
        }
    }

    let value = sort_value(serde_json::to_value(value)?);
    Ok(serde_json::to_vec(&value)?)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;

    fn provenance() -> ProtocolProvenance {
        ProtocolProvenance {
            actor: "agent:test".to_string(),
            tool: "bonhomme-test/1".to_string(),
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            source_revision: Some("abc123".to_string()),
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn protocol_version_has_a_stable_camel_case_shape() {
        assert_eq!(
            serde_json::to_value(CURRENT_PROTOCOL_VERSION).unwrap(),
            json!({"major": 1, "minor": 0})
        );
        assert!(SUPPORTED_PROTOCOL_RANGE.contains(CURRENT_PROTOCOL_VERSION));
        assert!(SUPPORTED_PROTOCOL_RANGE.contains(ProtocolVersion::new(1, 42)));
        SUPPORTED_PROTOCOL_RANGE
            .require_supported(ProtocolVersion::new(1, 42))
            .unwrap();
        assert!(!SUPPORTED_PROTOCOL_RANGE.contains(ProtocolVersion::new(2, 0)));
    }

    #[test]
    fn signing_payload_is_canonical_and_excludes_signature() {
        let mut batch = OperationBatch::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            7,
            vec![Operation::DeleteReference {
                reference_id: Uuid::from_u128(4),
            }],
            provenance(),
        );
        let unsigned = batch.signing_payload().unwrap();
        batch.signature = Some(DetachedSignature {
            algorithm: "ed25519".to_string(),
            key_id: "team-key".to_string(),
            value: "encoded-signature".to_string(),
        });
        assert_eq!(unsigned, batch.signing_payload().unwrap());
        assert!(!String::from_utf8(unsigned).unwrap().contains("signature"));
    }

    #[test]
    fn operation_batch_json_shape_is_golden_and_context_bound() {
        let batch = OperationBatch::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            7,
            vec![Operation::DeleteReference {
                reference_id: Uuid::from_u128(4),
            }],
            provenance(),
        );
        assert_eq!(
            serde_json::to_value(&batch).unwrap(),
            json!({
                "protocolVersion": {"major": 1, "minor": 0},
                "batchId": "00000000-0000-0000-0000-000000000001",
                "repositoryId": "00000000-0000-0000-0000-000000000002",
                "branchId": "00000000-0000-0000-0000-000000000003",
                "basePosition": 7,
                "operations": [{
                    "type": "DeleteReference",
                    "referenceId": "00000000-0000-0000-0000-000000000004"
                }],
                "provenance": {
                    "actor": "agent:test",
                    "tool": "bonhomme-test/1",
                    "createdAt": "2023-11-14T22:13:20Z",
                    "sourceRevision": "abc123"
                }
            })
        );
        batch
            .validate_context(Uuid::from_u128(2), Uuid::from_u128(3), 7)
            .unwrap();
        assert!(
            batch
                .validate_context(Uuid::from_u128(9), Uuid::from_u128(3), 7)
                .is_err()
        );
    }

    #[test]
    fn graph_snapshot_json_shape_is_golden_and_context_bound() {
        let snapshot = GraphSnapshot {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            repository_id: Uuid::from_u128(2),
            branch_id: Uuid::from_u128(3),
            position: 0,
            graph: SemanticGraph::default(),
            provenance: provenance(),
        };
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            json!({
                "protocolVersion": {"major": 1, "minor": 0},
                "repositoryId": "00000000-0000-0000-0000-000000000002",
                "branchId": "00000000-0000-0000-0000-000000000003",
                "position": 0,
                "graph": {"symbols": {}, "references": {}, "appliedOperations": []},
                "provenance": {
                    "actor": "agent:test",
                    "tool": "bonhomme-test/1",
                    "createdAt": "2023-11-14T22:13:20Z",
                    "sourceRevision": "abc123"
                }
            })
        );
        snapshot
            .validate_context(Uuid::from_u128(2), Uuid::from_u128(3), 0)
            .unwrap();
        assert!(
            snapshot
                .validate_context(Uuid::from_u128(2), Uuid::from_u128(9), 0)
                .is_err()
        );
    }

    #[test]
    fn future_v1_additive_fields_are_accepted_but_a_new_major_is_rejected() {
        let snapshot = GraphSnapshot {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            repository_id: Uuid::from_u128(2),
            branch_id: Uuid::from_u128(3),
            position: 0,
            graph: SemanticGraph::default(),
            provenance: provenance(),
        };
        let mut value = serde_json::to_value(snapshot).unwrap();
        value["protocolVersion"] = json!({"major": 1, "minor": 42});
        value["futureAdditiveField"] = json!({"ignoredByV1": true});

        let additive: GraphSnapshot = serde_json::from_value(value.clone()).unwrap();
        additive
            .validate_context(Uuid::from_u128(2), Uuid::from_u128(3), 0)
            .unwrap();

        value["protocolVersion"] = json!({"major": 2, "minor": 0});
        let incompatible: GraphSnapshot = serde_json::from_value(value).unwrap();
        assert!(incompatible.validate_envelope().is_err());
    }

    #[test]
    fn canonical_json_recursively_sorts_extension_keys() {
        let value = json!({"z": {"b": 2, "a": 1}, "a": 0});
        assert_eq!(
            String::from_utf8(canonical_json_bytes(&value).unwrap()).unwrap(),
            r#"{"a":0,"z":{"a":1,"b":2}}"#
        );
    }

    #[test]
    fn stable_symbol_identity_is_repeatable_and_domain_separated() {
        let identity = SymbolIdentityV1 {
            handler: "rust".to_string(),
            repository_relative_path: "src/lib.rs".to_string(),
            kind: "function".to_string(),
            qualified_name: "crate::parse".to_string(),
        };
        assert_eq!(identity.stable_id().unwrap(), identity.stable_id().unwrap());
        let mut other = identity.clone();
        other.handler = "typescript".to_string();
        assert_ne!(identity.stable_id().unwrap(), other.stable_id().unwrap());
    }

    #[test]
    fn legacy_migration_requires_explicit_context_and_emits_v1() {
        let context = LegacyBatchContext {
            batch_id: Uuid::from_u128(1),
            repository_id: Uuid::from_u128(2),
            branch_id: Uuid::from_u128(3),
            base_position: 0,
            provenance: provenance(),
        };
        let batch = migrate_legacy_operations(Vec::new(), context);
        assert_eq!(batch.protocol_version, CURRENT_PROTOCOL_VERSION);
        batch.validate_envelope().unwrap();
    }
}
