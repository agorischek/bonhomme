# Incremental graph materialization

Each branch keeps its newest graph checkpoint. A checkpoint is an optimization only; operation
records remain authoritative. Materialization applies the following checks before reusing one:

1. The checkpoint position must be non-negative and no newer than the requested branch position.
2. A SHA-256 fingerprint over the ordered operation ids, positions, and canonical operation payloads
   must match the exact prefix represented by the checkpoint.
3. The checkpoint envelope and format version must decode; its embedded protocol `GraphSnapshot`
   must support the reader's major version and match the authoritative repository, branch, and
   position supplied by the graph-cache row.
4. A separate SHA-256 graph fingerprint must match the canonical checkpoint graph.
5. The graph's applied-operation ids must exactly match the operation-prefix ids.
6. All graph invariants must validate.

If any check fails, Bonhomme records a typed rejection reason, replays from operation zero, and
replaces the bad checkpoint after a successful full materialization. A valid checkpoint is reused
and only the operation suffix is replayed. The suffix result is validated again before it is
returned or cached.

`MaterializedBranch` and `MaterializedGraph` expose `materialization` evidence: total operation
count, checkpoint operation count, replayed suffix count, whether the prefix fingerprint and graph
invariants were verified, and any rejected-checkpoint reason. Cache status distinguishes an exact
`HIT`, an `INCREMENTAL` suffix replay, and a full `MISS`.

Rendered files are regenerated with the currently configured plugin even on a graph-cache hit. This
avoids serving a stale projection after plugin code or formatting behavior changes; the checkpoint
accelerates semantic replay without weakening current-render equivalence.

## Release acceptance benchmark

The release-mode harness defaults to one million operations and one million live symbols:

```sh
cargo run --release -p bonhomme-engine --example incremental_materialization_scale -- 1000000 1000000
```

It prints JSON containing build, full-replay, checkpoint-encoding, checkpoint-verification,
suffix-replay, and total wall times; cache/replay counts; prefix verification and exact-equivalence
flags; serialized operation-log and checkpoint sizes; and a portable memory proxy. The proxy is
explicitly labeled as serialized working set plus the shallow operation vector and is not presented
as peak RSS. On macOS, capture OS-reported peak resident memory as an additional acceptance datum:

```sh
/usr/bin/time -l cargo run --release -p bonhomme-engine --example incremental_materialization_scale -- 1000000 1000000
```

Use smaller counts only for smoke testing. A release acceptance report is valid only when
`operationCount >= 1000000`, `prefixFingerprintVerified` and `exactFullReplayEquivalence` are true,
`renderedTreeEquivalence` is true, and `replayedOperations` is `1`.
