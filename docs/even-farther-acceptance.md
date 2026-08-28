# Production Expansion Acceptance Contract

This document defines executable completion evidence for the seven production-expansion tracks. A
track is not complete because an API or design exists; it is complete only when the listed behavior
is exercised by automated tests or a reproducible command.

## 1. Git bridge and shadow mode

- Import a named Git commit without mutating the working tree.
- Export a Bonhomme branch to a directory whose Git tree ID is deterministic.
- Shadow-evaluate a real Git merge and emit a machine-readable report without moving refs.
- Demonstrate byte-identical round trips for the supported fixture corpus and explicit degradation
  records for files handled as blobs.

## 2. Git-vs-Bonhomme benchmark corpus

- Keep versioned fixtures for independent same-file edits, rename-plus-edit, move-plus-caller-edit,
  delete-versus-reference, duplicate semantic identity, and formatting churn.
- Run both Git and Bonhomme from the same immutable fixture manifest.
- Emit deterministic JSON containing clean-merge rate, conflict count, validation outcome, elapsed
  time, and unsafe/incorrect outcome count.
- Fail the benchmark verifier if an expected semantic outcome changes without a fixture update.

## 3. Storage conformance and failure injection

- Run one backend-neutral behavioral suite against Turso and Postgres.
- Cover idempotency, branch position allocation, conflict retention, cache replacement, and snapshot
  replacement.
- Inject a failure during every multi-write operation and prove the pre-operation state is retained.
- Run the Postgres suite in CI against an isolated service database.

## 4. Versioned semantic protocol and plugin capabilities

- Wrap persisted/exported semantic data in a protocol envelope with an explicit version.
- Reject unsupported major versions and accept compatible minor versions.
- Give every language plugin a serializable capability declaration covering round-trip fidelity,
  identity, references, recovery, validation, and incremental parsing.
- Snapshot-test the protocol JSON shape and capability registry.

## 5. Durable job execution

- Persist jobs and enforce idempotent enqueue per tenant.
- Lease jobs atomically with owner and expiry, heartbeat leases, reclaim expired leases, and apply
  bounded retry/backoff policy.
- Support cancellation and terminal result/error records.
- Recover correctly after a worker or API process restarts.
- Exercise queue semantics against every production storage backend.

## 6. Incremental materialization and scale

- Reuse a verified graph checkpoint and replay only its suffix when a branch advances.
- Prove incremental and full replay produce identical graphs and rendered trees.
- Invalidate checkpoints when their operation prefix fingerprint differs.
- Benchmark at least one million operations/symbol-scale synthetic state and record time, peak memory,
  cache hit/replay counts, and serialized checkpoint size.

## 7. Production trust boundaries

- Refuse non-loopback serving without configured credentials.
- Authenticate bearer credentials without logging them; authorize read/write roles and tenant access.
- Enforce request size, timeout, and concurrency limits.
- Attach and propagate request IDs and expose request/error/latency metrics.
- Maintain a durable, hash-chained local audit record for API and direct CLI mutations. Treat it as
  tamper-evident rather than immutable, and require production deployments to configure automatic
  export/anchoring into an external WORM system. Document backup, restore, credential rotation, and
  incident-response procedures.
- Run secret, dependency, migration, cross-platform, and production-configuration checks in CI.

## Completion gate

All tracks must pass their focused tests plus `cargo fmt --all -- --check`, strict workspace Clippy,
all workspace targets, frontend lint/build/audit, storage conformance, dependency audit, repository
policy, and a secret scan of history and the complete transfer set.
