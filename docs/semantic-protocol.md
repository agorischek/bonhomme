# Semantic protocol

Bonhomme's crate version and semantic protocol version are intentionally independent. The crate may
release implementation changes without invalidating stored operations. Protocol v1.0 covers the
serialized `Operation` variants, their graph contracts, versioned operation batches, graph
snapshots, provenance, detached signatures, and language-handler capability reports.

## Compatibility rules

- Major versions are strict. A v1 reader rejects a v2 batch or snapshot before replay.
- Minor versions are additive. Existing field and enum spellings cannot change within a major
  version, and new fields must have safe defaults for older documents. This reader accepts any v1
  minor and continues to emit v1.0; a semantic or enum-breaking change requires v2.
- Bare operation arrays are legacy data. Migrating them to v1 requires explicit repository, branch,
  base-position, batch-identity, and provenance context; the core does not guess those values.
- `OperationBatch::signing_payload` is the canonical compact JSON representation with the detached
  signature omitted. Object keys are sorted recursively and array order is retained.
- A graph snapshot is accepted only if its version is supported and all graph invariants validate.

The current reader range is exposed as `SUPPORTED_PROTOCOL_RANGE`; writers emit
`CURRENT_PROTOCOL_VERSION`.

These envelopes are enforced at two production boundaries today:

- graph-cache rows persist a `GraphSnapshot` and verify its protocol version, repository, branch,
  position, graph fingerprint, applied-operation prefix, and graph invariants before reuse; and
- `bonhomme git export` emits an `operationBatch` manifest alongside the deterministic Git tree
  report, binding the exported semantic operation sequence to its repository and branch.

The relational operation log remains the replay authority and retains its legacy per-operation row
shape. Consumers should use the Git export manifest for versioned interchange rather than treating
raw database payload columns as a public wire format.

## Identity

`SymbolIdentityV1` defines the recommended UUIDv5 input for new plugins: handler, normalized
repository-relative path, symbol kind, and qualified name. It is deterministic and domain-separated.
Fresh imports are path-sensitive. A plugin may preserve an existing identity across a rename or move
when graph-anchored recovery can prove the match, but it must report that behavior in its capability
manifest rather than promising global rename stability.

Existing plugins keep their established UUIDv5 seeds so previously stored repositories remain
replayable. Changing a plugin's identity seed is a data migration, not an implementation detail.

## Operation contracts

Every `Operation` exposes machine-readable preconditions and postconditions through
`Operation::contract()`. The replay engine remains authoritative and enforces these conditions. The
contract vocabulary lets remote workers, audit tooling, and future protocol implementations reason
about failures without parsing human-readable error messages.

## Capability reports

Every `LanguagePlugin` can return `PluginCapabilities`, and `HandlerRegistry::handler_manifests()`
returns named reports in routing-priority order. Reports independently describe:

- source fidelity;
- symbol granularity;
- identity stability;
- reference extraction;
- rename and move recovery;
- validation strength;
- incremental parsing; and
- known limitations.

Third-party plugins that have not adopted the declaration API remain source-compatible, but report
`undeclared` rather than inheriting an unsafe optimistic default. The built-in language and fallback
handlers publish explicit reports. `verified` is reserved for a capability covered by the shared
conformance corpus; implemented plugin-local behavior is reported as `supported` or `experimental`
until that evidence exists.

## Signatures and provenance

Operation batches carry immutable actor/tool/timestamp/source-revision provenance and optional
detached signature metadata. The core defines the bytes to sign but deliberately does not manage
private keys or choose a signature algorithm. A deployment must define allowed algorithms, key
lookup, revocation, and verification policy before accepting signed batches as trusted.
