# Semantic Conflict Resolution Plan

Bonhomme must give an agent a well-defined way to get unstuck when a slice apply or merge
encounters a semantic conflict. The desired model is intentionally close to Git conflict
resolution: the original attempt is preserved, the system enters an explicit conflict state, the
agent receives enough context to make a resolution edit, and the resolved work resumes through the
normal apply/merge path.

This document describes the target design, the implemented prototype shape, and the remaining gaps.

Implementation status: the current prototype implements the core CLI/API workflow. Merge and stale
slice-apply conflicts are persisted as conflict records, checked out into
`base/`/`current/`/`proposed/`/`resolved/` workspaces, resolved from ordinary source edits, validated,
and appended as resolution changesets. The dry-run and real Codex swarm runner use this path instead
of replacing conflicted branches.

## Current State

Bonhomme already has useful pieces:

- Stored slices record repository, branch, base position, and root symbols.
- Slice apply recovers operations from edited source and checks stale branches before append.
- Merge returns a structured `MergeResult` with `SAFE_MERGE` or `CONFLICT`.
- Conflict records include reason, source operation, target operation, optional symbol, and detail.
- Failed merges do not mutate the target branch.
- Expected merge and stale slice-apply conflicts are persisted as first-class conflict records.
- `bonhomme conflict checkout` materializes base/current/proposed/resolved projections.
- `bonhomme conflict resolve` recovers semantic operations from `resolved/`, checks recovery
  faithfulness, validates the rendered source, appends a resolution changeset, and marks the conflict
  resolved.
- The API exposes the same list/show/checkout/resolve/abandon workflow.
- The swarm runner resolves semantic conflicts through the core conflict workflow.

The remaining work is breadth and hardening:

- Broaden resolution tests across destructive edits, update/delete conflicts, reference retargeting,
  branch advancement during resolution, and non-TypeScript handlers.
- Keep closing import/recover parity gaps (see `reports/parser-unification-plan.md`) so more edits can
  land faithfully instead of returning `StillConflicting`.
- Add richer review UI for open/resolved conflicts beyond branch status and merge logs.
- Extend beyond the v1 direct branch-to-target merge shape.

## Design Goals

- Preserve the original conflicting attempt as provenance.
- Represent conflict state durably and queryably.
- Give agents deterministic, minimal resolution workspaces.
- Let agents resolve conflicts by editing ordinary source files.
- Recover the resolution back into semantic operations.
- Guarantee recovery faithfulness: accept a resolution only when its recovered operations re-render to
  the exact edited source, so no agent edit is ever silently dropped.
- Validate and append the resolution as a new changeset.
- Keep conflict resolution language-plugin-aware but core-orchestrated through a single creation path.
- Support both human and agent workflows through the same primitives.
- Avoid hidden source anchors as the required correctness mechanism.

## Non-Goals

- Full arbitrary branch DAG support in the first implementation.
- Multi-party collaborative editing inside one conflict workspace.
- Perfect automatic conflict resolution for all semantic conflict classes.
- Replacing existing merge analysis; this plan builds on it.

## Orchestration Boundary

Conflicts must be created in exactly one place. Today slice apply is orchestrated in the app layer and
bails on conflict in two independent sites, while merge conflict shaping lives in the engine; nothing
guarantees the two agree on what a conflict is or does.

This plan introduces a single core entry point that, on detecting `CONFLICT`, persists a
`ConflictRecord`, stores the proposed operations without appending them, and renders the workspace.
Slice apply, merge, the CLI, the API, and the demo all route through it. "Core-orchestrated" in the
design goals means this entry point lives in `bonhomme-engine`, not that every caller reimplements it.

The language plugin stays responsible only for language-specific work (import, render, recover, diff,
validate). Conflict lifecycle, persistence, and workspace materialization are core.

## Concepts

### Conflict

A durable record that says a slice apply or merge could not safely append operations.

Required fields:

- `id`
- `repository_id`
- `kind`: `slice_apply` or `merge`
- `state`: `open`, `resolving`, `resolved`, `abandoned`
- `source_branch_id`, `target_branch_id`
- `source_branch_name`, `target_branch_name`: denormalized for provenance, so the record stays
  inspectable even if a branch is later deleted
- `slice_id`, when the conflict came from slice apply
- `source_changeset_id`, when source operations already exist
- `base_position`
- `target_position`
- `conflicts`: structured `MergeConflict[]`
- `proposed_operations`: the source attempt's recovered operations, stored but not appended
- `created_by`
- `created_at`
- `resolved_by`, `resolved_at`, optional
- `resolution_changeset_id`, optional

For `kind = slice_apply` the conflict lives within a single branch: `source_branch_id` equals
`target_branch_id`, `source_changeset_id` is null (the proposed operations are not yet a changeset —
that is the core invariant), and the proposed operations are carried in `proposed_operations`. For
`kind = merge`, source and target differ and `source_changeset_id` may reference existing source work.

### Conflict Workspace

A checked-out directory produced from a conflict. It is analogous to a Git working tree with conflict
markers, but uses separate projections instead of inline marker syntax.

Suggested layout:

```text
.bonhomme/conflicts/<conflict-id>/
  .bonhomme-conflict.json
  base/
    src/OrderService.ts
  current/
    src/OrderService.ts
  proposed/
    src/OrderService.ts
  resolved/
    src/OrderService.ts
  CONFLICT.md
```

`resolved/` is initialized from `current/` only (see Resolved View for why proposed intent is not
auto-merged).

The workspace machinery generalizes the existing slice workspace in
`crates/bonhomme/src/cli/slice_workspace.rs`: manifest checkout, `safe_relative_path` traversal
guards, utf-8/base64 encoding detection, an active-pointer, and rejection of unlisted files. Two
changes are required when reusing it: support multiple projection roots (`base/`, `current/`,
`proposed/`, `resolved/`) under one manifest, and lift the current "add-file slices are not supported
yet" restriction, since a resolution may legitimately add a file.

### Resolution Changeset

A normal changeset appended to the affected branch that represents the conflict resolution. It
contains recovered semantic operations from `resolved/` against the conflict base/current context
and carries a `ConflictResolutionAttachment`.

### Recovery Faithfulness

The resolution flow turns edited `resolved/` source back into operations via the language plugin's
recover/diff path. That write path does not yet model every construct the import read path models: per
`reports/parser-unification-plan.md`, edits to unmodeled kinds (today, file preamble and import
statements in TypeScript) are silently dropped. A dropped edit would void an agent's resolution while
reporting success — the exact failure this feature exists to prevent.

Recovery faithfulness is therefore a hard invariant: a resolution is accepted only if the recovered
operations, applied to the destination graph and re-rendered, reproduce `resolved/` exactly (modulo
formatting). Any divergence is surfaced as `StillConflicting` with a diagnostic, never silently
accepted. This check is what makes the rest of the design safe on top of a write path that is not yet
at parity with the read path.

## State Machine

```text
no_conflict
  -> open_conflict
  -> resolving
  -> resolved
  -> resumed_merge
  -> landed

open_conflict
  -> abandoned
```

The persisted `state` column is coarse: `open`, `resolving`, `resolved`, `abandoned`. The remaining
labels are derived, not stored: `no_conflict` is the absence of a record; `resumed_merge` and `landed`
are derived from `resolution_changeset_id` plus merge completion. `resolving` is set when a workspace
is checked out and cleared back toward `open`/`resolved` by the resolve outcome.

Operationally:

1. Apply or merge detects `CONFLICT`.
2. Bonhomme persists an `open` conflict.
3. Bonhomme renders a conflict workspace.
4. Agent edits `resolved/`.
5. Bonhomme applies the resolution.
6. Bonhomme marks the conflict `resolved`.
7. Bonhomme retries or resumes the merge.

## Workflows

### Slice Apply Conflict

This happens when an agent created a slice at branch position `N`, then the branch advanced before
the agent applies the edited slice.

Target behavior:

1. Recover operations from edited slice against the stored slice base graph.
2. Analyze recovered operations against branch operations since `base_position`.
3. If safe, append as today.
4. If conflicting:
   - Persist `Conflict(kind=slice_apply)` through the single core creation path.
   - Store recovered proposed operations without appending them.
   - Render base/current/proposed/resolved projections.
   - Return `ApplySliceResult::Conflict`.
5. Agent edits `resolved/`.
6. `bonhomme conflict resolve <conflict-id>` recovers operations from `resolved/` against current
   branch state and appends a resolution changeset, subject to the faithfulness check and the
   empty-resolution guard (see Recovery Algorithm).

Important invariant: the original proposed operations are not appended to the branch until a safe
resolution exists.

### Merge Conflict

This happens when a source branch has operations that cannot apply cleanly to a target branch.

Target behavior:

1. Analyze source operations against target operations since the source fork point.
2. If safe, merge as today.
3. If conflicting:
   - Persist `Conflict(kind=merge)` through the single core creation path.
   - Render base/current/proposed/resolved projections.
   - Return the existing `MergeResult` struct, extended with `conflict_id: Option<Uuid>` (there is no
     `MergeResult` enum today; add the field rather than restructuring).
4. Agent edits `resolved/`.
5. `bonhomme conflict resolve <conflict-id>` creates a resolution changeset, subject to the
   faithfulness check and the empty-resolution guard (see Recovery Algorithm).
6. Bonhomme appends the resolution to the target as a merge-resolution changeset and marks the merge
   complete.

Preferred v1 strategy: append the resolution to the target as a merge-resolution changeset. This
matches Git's behavior: the target history records both the attempted merge and the resolution. It is
safe only because the faithfulness check guarantees the appended operations reproduce exactly what the
agent approved; a resolution that carries none of the source intent is a "keep target, drop source"
result and requires explicit acknowledgement rather than silently landing as success.

## Resolution Workspace Semantics

### Base View

The materialized graph/files at the common semantic base:

- For merge: target branch at `source.base_position`.
- For stale slice apply: branch at `stored_slice.base_position`.

### Current View

The materialized graph/files currently present on the destination branch.

### Proposed View

The source attempt rendered independently:

- For merge: base plus source branch operations.
- For slice apply: base plus recovered proposed operations.

### Resolved View

The editable projection. The agent edits only files under `resolved/`.

Initial content rules:

- Prefill `resolved/` from `current/` only. Do not auto-merge proposed additions, even when they look
  non-conflicting. Any prefilled edit the recover/diff write path cannot model would be silently
  dropped at resolution time (see Recovery Faithfulness), so the conservative prefill is also the safe
  one.
- List every proposed operation in `CONFLICT.md` so the agent carries intent forward deliberately.
- Never silently discard proposed intent. If proposed work is not carried into `resolved/`, it stays
  listed in `CONFLICT.md`, and a resolution that carries none of it is treated as an explicit "keep
  current, drop source" decision that must be acknowledged (see the empty-resolution guard in the
  Recovery Algorithm).

## Data Model

Add tables or equivalent storage records:

```sql
CREATE TABLE conflicts (
  id UUID PRIMARY KEY,
  repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  -- Branch FKs are nullable + SET NULL so a resolved/abandoned conflict survives branch deletion;
  -- the denormalized *_branch_name columns keep it inspectable as provenance.
  source_branch_id UUID REFERENCES branches(id) ON DELETE SET NULL,
  target_branch_id UUID REFERENCES branches(id) ON DELETE SET NULL,
  source_branch_name TEXT NOT NULL,
  target_branch_name TEXT NOT NULL,
  slice_id UUID REFERENCES slices(id) ON DELETE SET NULL,
  source_changeset_id UUID REFERENCES changesets(id) ON DELETE SET NULL,
  base_position BIGINT NOT NULL,
  target_position BIGINT NOT NULL,
  conflicts JSONB NOT NULL,
  proposed_operations JSONB NOT NULL,
  resolution_changeset_id UUID REFERENCES changesets(id) ON DELETE SET NULL,
  acknowledged_drop BOOLEAN NOT NULL DEFAULT false,
  created_by TEXT NOT NULL,
  resolved_by TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  resolved_at TIMESTAMPTZ
);
```

Retention and provenance: an `open` or `resolving` conflict blocks deletion of the branches it
references, so an in-flight resolution cannot lose its base/current/proposed views. Once `resolved` or
`abandoned`, the record is retained even if a referenced branch is later deleted — the branch FKs go
null but `source_branch_name`/`target_branch_name` and the in-row `proposed_operations` preserve the
provenance, with the caveat that base/current/proposed source views can no longer be re-materialized
once the underlying branch operation log is gone.

For Turso/local storage, mirror the same shape with JSON text columns (`conflicts` and
`proposed_operations` as TEXT via `serde_json::to_string`, booleans as integers), and keep the two
backends in sync by hand as with the existing tables.

Attachments:

- `ConflictAttachment` on task or changeset for review UI.
- `ConflictWorkspaceAttachment` records workspace path and file manifest.
- `ConflictResolutionAttachment` records resolved files, recovered operations, validation results,
  and whether the merge was resumed automatically.

## Core Types

Add explicit result types instead of returning plain errors for expected conflicts.

```rust
pub enum ApplySliceResult {
    Applied {
        state: DemoState,
        changeset_id: Uuid,
        appended_operations: Vec<OperationRecord>,
    },
    Conflict {
        conflict: ConflictRecord,
        workspace: Option<ConflictWorkspace>,
    },
}

pub struct ConflictRecord {
    pub id: Uuid,
    pub kind: ConflictKind,
    pub state: ConflictState,
    pub source_branch: Branch,
    pub target_branch: Branch,
    pub base_position: i64,
    pub target_position: i64,
    pub conflicts: Vec<MergeConflict>,
    pub proposed_operations: Vec<Operation>,
}

pub enum ResolveConflictResult {
    Resolved {
        conflict: ConflictRecord,
        changeset_id: Uuid,
        appended_operations: Vec<OperationRecord>,
        resumed_merge: Option<MergeResult>,
    },
    StillConflicting {
        conflict: ConflictRecord,
        // Why the resolution did not land: unresolved semantic conflicts, regions of `resolved/`
        // that failed the faithfulness round-trip, a re-conflict from the branch advancing during
        // resolve, or a toolchain rejection.
        reason: StillConflictingReason,
        conflicts: Vec<MergeConflict>,
        unfaithful_files: Vec<String>,
        workspace: ConflictWorkspace,
    },
}
```

Notes:

- `ConflictRecord` here is the in-memory runtime view returned to callers; the persisted shape is the
  superset in Data Model (it adds `repository_id`, `created_by`/`created_at`, `resolution_changeset_id`,
  denormalized branch names, etc.). For `kind = slice_apply`, `source_branch` and `target_branch` are
  the same branch.
- There is no `MergeResult` enum to add a variant to: `MergeResult` is a struct. Extend it with
  `conflict_id: Option<Uuid>` rather than restructuring it.
- `conflict resolve` takes an explicit acknowledgement flag (e.g. `accept_drop: bool`) that satisfies
  the empty-resolution guard; without it, a drop-source resolution returns `StillConflicting`.

## CLI Surface

```sh
bonhomme conflict list --repo <repo>
bonhomme conflict show <conflict-id> --json
bonhomme conflict checkout <conflict-id> --out .bonhomme/conflicts/<id>
bonhomme conflict resolve <conflict-id> --dir .bonhomme/conflicts/<id>/resolved --agent <agent> [--accept-drop]
bonhomme conflict abandon <conflict-id> --reason "<reason>"
```

Slice apply and merge should print conflict IDs when they cannot complete:

```sh
bonhomme slice apply --repo demo --branch agent-011
# CONFLICT 3e4d... checked out to .bonhomme/conflicts/3e4d...

bonhomme merge --repo demo --source agent-011 --target main
# CONFLICT 7a2b... checked out to .bonhomme/conflicts/7a2b...
```

## API Surface

```text
POST /api/conflicts/:id/checkout
POST /api/conflicts/:id/resolve   { dir, agent, acceptDrop? }
POST /api/conflicts/:id/abandon
GET  /api/conflicts
GET  /api/conflicts/:id
```

`resolve` returns the structured `ResolveConflictResult`, so an API caller sees `StillConflicting`
(with the faithfulness/toolchain diagnostic) the same way the CLI does.

Demo-specific endpoints should call these primitives rather than deleting/recreating branches.

## Agent Contract

When an agent receives a conflict, Bonhomme should give it a precise instruction block:

```text
You are resolving Bonhomme conflict <id>.

Read:
- CONFLICT.md
- base/<path>
- current/<path>
- proposed/<path>

Edit only files under resolved/.
Preserve current behavior unless CONFLICT.md names proposed intent to carry forward.
Do not edit .bonhomme-conflict.json.
When done, run:
  bonhomme conflict resolve <id> --dir resolved --agent <agent-name>

If resolve reports STILL_CONFLICTING, read the diagnostic. It names one of: unresolved semantic
conflicts, regions of resolved/ that could not be recovered faithfully (fix those edits and retry),
or a toolchain rejection. To intentionally keep current behavior and drop the proposed change, rerun
with --accept-drop.
```

The agent should not guess by deleting and recreating its branch. The branch remains part of the
provenance.

## Recovery Algorithm

Resolution apply does not diff `resolved/` against the original slice base. It recovers the semantic
delta from the destination graph as it exists *now*, then proves the recovery is faithful before it is
allowed to land.

Algorithm:

1. Load `ConflictRecord`.
2. Re-read the destination branch head. If the branch advanced past the recorded `target_position`
   since checkout, the conflict workspace is stale: refresh `current/` from the new head, re-recover
   `proposed_operations` if needed, and re-render `CONFLICT.md` before continuing. The destination base
   for recovery is always the live head, never the recorded `target_position`.
3. Materialize the destination head into `current_graph`.
4. Render or load `resolved/` files.
5. Ask the language plugin to recover operations from `current_graph` to `resolved_files`.
6. Faithfulness check (mandatory): apply the recovered operations to `current_graph`, render the
   result, and compare against `resolved/` modulo formatting. If they differ, the write path failed to
   model an edit the agent made (see Recovery Faithfulness). Do not append; return `StillConflicting`
   with a diagnostic naming the files/regions that did not round-trip.
7. Empty-resolution guard: if the recovered operation set carries none of the proposed intent
   (typically because `resolved/` still equals `current/`), this is a "keep current, drop source"
   resolution. Accept it only when the caller passes an explicit acknowledgement (`--accept-drop` /
   `acceptDrop: true`); otherwise return `StillConflicting`.
8. Analyze recovered resolution operations against destination operations since the head used in
   step 3. If they conflict (the branch moved again mid-resolve), return `StillConflicting`.
9. Validate operation replay, render, and the language toolchain (compiler/linter). A compiler
   rejection returns `StillConflicting`, not an opaque error.
10. Append operations as a resolution changeset carrying a `ConflictResolutionAttachment`.
11. Mark conflict resolved.
12. If the conflict came from merge, append the resolution to the target as a merge-resolution
    changeset and mark the merge complete.

For v1, direct target append is simpler and more Git-like. It is only safe because step 6 guarantees
the appended operations reproduce exactly what the agent approved; without that check, direct append
could land a silently-truncated resolution.

### Dependency on parser unification

Steps 5–6 depend on the recover/diff write path modeling every kind the import read path models. Until
`reports/parser-unification-plan.md` lands (or at minimum its parser-parity guardrail test), the
faithfulness check in step 6 is the backstop that converts a silent drop into an explicit
`StillConflicting`. This plan must not ship Phase 3 without step 6.

## Conflict Classes

Initial support should cover:

- duplicate symbol name
- duplicate symbol ID
- duplicate reference ID
- overlapping symbol write
- overlapping reference write
- reference to deleted symbol
- validation rejected
- compiler rejected

Each class needs a `CONFLICT.md` rendering with:

- reason
- affected symbols/references
- current operation summary
- proposed operation summary
- suggested resolution shape

## UI Plan

Workbench should expose conflicts as first-class items:

- Agent card state: `conflicted`, not merely `awaiting merge`.
- Conflict inspector tab:
  - conflict reason
  - current/proposed side-by-side rendered source
  - proposed operation list
  - resolution status
- Operation log should show conflict creation and resolution changesets.
- Real swarm should show an agent moving through `editing -> applying -> conflicted -> resolving -> landed`.

## Migration From Current Demo Repair

The current demo runner has a temporary path that does not actually resolve the conflict — it
sidesteps it by generating a *new* method on a replaced branch (e.g. a `...Resolution` method) rather
than reconciling the conflicting change. So Phase 5 introduces real resolution for the first time, not
a swap of one resolution mechanism for another; the deterministic-conflict smoke test is the proof that
resolution works end to end.

```text
merge conflict -> replace agent branch -> create fresh slice from main -> merge fresh branch
```

Replace it with:

```text
merge conflict -> conflict checkout -> agent edits resolved/ -> conflict resolve -> merge resumes
```

Specific tasks:

- Remove `replaceBranch` from the demo API once conflict checkout exists.
- Stop using `delete_branch_by_name` as conflict repair.
- Keep branch deletion only if there is a separate explicit branch-management command.
- Make `codex-swarm.mjs` call `conflict checkout` and `conflict resolve`.

## Implementation Phases

### Phase 1: Durable Conflict Records and a Single Creation Path

- Add `ConflictRecord` model and the `conflicts` table in Postgres and Turso.
- Introduce one core entry point in `bonhomme-engine` that creates a conflict: persist the record,
  store proposed operations, denormalize branch names, mutate nothing on the target. Route both slice
  apply and merge through it.
- Replace the two app-layer `bail!` sites (`crates/bonhomme/src/demo/agents.rs`,
  `crates/bonhomme/src/cli/commands.rs`) with `ApplySliceResult::Conflict` sourced from that path.
- Extend the `MergeResult` struct with `conflict_id: Option<Uuid>`.
- Add tests that conflicts persist and can be listed, and that slice apply and merge produce identical
  conflict records through the shared path.

### Phase 2: Conflict Workspace Checkout

- Implement conflict materialization for base/current/proposed/resolved.
- Generate `.bonhomme-conflict.json`.
- Generate `CONFLICT.md`.
- Add `bonhomme conflict checkout`.
- Add API checkout endpoint.

### Phase 3: Resolution Apply

- Implement `bonhomme conflict resolve`.
- Recover resolution operations against the live destination head, refreshing a stale workspace first.
- Enforce the faithfulness check: recovered operations must re-render to `resolved/` exactly, or return
  `StillConflicting` naming the files that did not round-trip. This is the backstop for the parser
  write-path gap and is non-optional.
- Enforce the empty-resolution guard: a drop-source resolution requires `--accept-drop`.
- Append `ConflictResolutionAttachment`; mark conflict resolved.
- Validate replay, render, and toolchain; a compiler rejection returns `StillConflicting`.
- Return `StillConflicting` instead of failing opaquely whenever the resolution is not enough.

Prerequisite: do not ship Phase 3 without the faithfulness check (step 6 of the Recovery Algorithm). It
assumes recover/diff parity with import is either achieved (`reports/parser-unification-plan.md`) or
backstopped by that check.

### Phase 4: Merge Resume

- For merge conflicts, append resolution to target and mark merge resolved.
- Record the original source branch and proposed operations as provenance.
- Add UI state for `resolved` and `landed`.

### Phase 5: Agent Integration

- Update `bonhomme agents` instructions to describe conflict resolution.
- Update Codex swarm runner to use conflict checkout/resolve.
- Remove demo branch replacement repair.
- Add dry-run and real-agent smoke tests.

### Phase 6: Rich Review UI

- Add conflict inspector tab.
- Show base/current/proposed/resolved source.
- Show structured semantic conflict list.
- Add controls for checkout, resolve, abandon, and retry.

## Test Plan

Unit tests:

- conflict record storage round trip
- conflict checkout manifest generation
- merge conflict persists without target mutation
- stale slice apply conflict persists without branch mutation
- resolution append marks conflict resolved
- still-conflicting resolution remains open
- slice apply and merge create equivalent conflict records through the single core path
- faithfulness check rejects an unfaithful resolution: an edit to an unmodeled kind (e.g. an import
  statement) returns `StillConflicting` instead of being silently dropped
- resolve rebases onto a destination head that advanced after checkout
- empty / drop-source resolution returns `StillConflicting` without `--accept-drop` and succeeds with it
- resolved/abandoned conflict survives deletion of a referenced branch (provenance retained)

Integration tests:

- two agents add duplicate method name; second resolves by renaming/carrying intent
- update/delete conflict resolved by keeping update or delete
- reference-to-deleted-symbol conflict resolved by removing or retargeting reference
- compiler-rejected resolution remains open
- resolution that edits file imports/preamble either lands faithfully or returns `StillConflicting`
  (never silently drops the edit)
- conflict resume validates final rendered files

Agent smoke tests:

- dry-run swarm with intentional conflicts
- real Codex swarm with one deterministic conflict
- agent resolves without branch replacement

## Resolved Decisions

- Merge resolution appends directly to the target as a merge-resolution changeset for v1. This is safe
  only because of the faithfulness check; a synthetic resolution branch is deferred.
- `resolved/` prefills from `current/` only; proposed intent is never auto-merged (see Resolved View).
- Core generates `CONFLICT.md` from operation summaries for v1; plugin-specific renderers are deferred.
- Conflict workspaces default to `.bonhomme/conflicts/<id>/`, with an optional `--out` override,
  mirroring slice checkout.

## Open Questions

- How should Bonhomme represent "source operation superseded by resolution operation" in history?
- Should an `--accept-drop` resolution record the dropped proposed operations as an explicit
  abandonment marker on the source branch, or only as conflict provenance?

## Acceptance Criteria

This feature is done when:

- Expected apply/merge conflicts return structured conflict records, not opaque errors.
- Conflicts are created through a single core path shared by slice apply, merge, CLI, and API.
- A conflict can be checked out, edited, resolved, and resumed from CLI.
- The same flow is available over API.
- The original conflicting attempt remains inspectable.
- The target branch is not mutated until a safe resolution is appended.
- A resolution is appended only if its recovered operations re-render to `resolved/` exactly;
  otherwise it stays open. No agent edit is silently dropped at resolution time.
- A resolution that discards all proposed intent requires explicit acknowledgement.
- The demo runner resolves conflicts through the core conflict workflow, not branch replacement.
- Documentation and agent instructions describe the resolution contract clearly enough for an agent
  to recover without human intervention in common cases.
