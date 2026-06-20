# Plan: Structural Identity Recovery (drop in-text symbol comments)

**Status:** in progress. P0-P3 are implemented; stale-base handling is next.
**Companion reading:** [core-premise.md](core-premise.md) (why files are projections),
[related-work.md](related-work.md) (Unison / structured-merge lineage).

## Goal

Stop carrying symbol identity in the rendered text. Today every rendered symbol
embeds a `/* bonhomme:symbol=<uuid> */` comment, and the agent must reproduce
those UUIDs verbatim when it hands a slice back. That is token-expensive and
fragile (a dropped or duplicated comment silently mis-attributes an edit).

Instead, hand the agent **clean, idiomatic TypeScript with no bonhomme metadata**,
and recover identity on the way back in by **matching the edited AST against the
authoritative graph** the slice was rendered from. Keep an *optional* explicit
anchor only for the genuinely ambiguous cases, so the system never has to guess.

Success looks like:

- A rendered slice contains no `bonhomme:symbol=` / `bonhomme:file=` comments.
- `slice apply` recovers the correct `Create/Update/Delete` operations from clean
  edited text, identical to what the comment-based path produces today.
- Identity survives renames, body rewrites, adds, and deletes within a slice's
  scope, deterministically, with the `tsc` gate as the safety net.

## Why (philosophy, briefly)

The premise is that **the operation log is authoritative and files are
compatibility output**. Comment-carried identity violates that: it makes the
*file* load-bearing state and forces the agent to do the system's bookkeeping.
Recovering identity structurally uses the log's authority to reconstruct what the
agent started from and infers the semantic operations from a meaning-to-meaning
diff — which is what "store the meaning, not the text" was supposed to be. This is
also the Unison end of the design space: identity is structural, never a text
annotation.

## Current state (post-oxc)

- `bonhomme-ts::parse` now parses with oxc (`with_program`, AST walk) into
  `ParsedFile { file_symbol_id, classes: [ParsedClass{ symbol_id, name, methods }],
  functions }`. Identity is *still* read from comments via
  `oxc_parse::find_symbol_id` / `find_file_symbol_id`.
- Full branch rendering still emits `bonhomme:symbol=` / `bonhomme:file=`
  comments for the legacy two-file diff path. Stored slice rendering is clean.
- `diff_slice(original, modified)` is **stateless**: it parses two supplied text
  blobs and matches by id-then-name. The new `ensure_unique_symbol_ids` guard
  rejects a slice that reuses an id.
- Slice provenance is persisted. `slice create` records the branch, base operation
  position, and root symbols, then returns a slice ID. `slice apply --slice-id`
  recovers against that stored base graph; `--original`/`--modified` remains as a
  legacy two-file diff path.
- The engine can already materialize the graph at any point:
  `collect_branch_operations(branch, Some(base_position))` + `materialize`.

## Target architecture

The round-trip becomes stateful and graph-anchored:

```text
render_slice(graph @ base_position, scope)  ->  clean TS (no metadata)   [+ persist slice provenance]
        |                                                 |
   agent edits the clean text                             |
        v                                                 v
apply(slice_id, edited_text)  ->  re-materialize graph @ base_position, scope
                              ->  oxc-parse edited_text
                              ->  STRUCTURAL MATCH edited AST <-> graph subtree
                              ->  Create / Update / Delete / reference ops
                              ->  re-render + tsc gate  (existing safety net)
```

Key shift: identity is recovered by matching the edited AST **against the graph
snapshot the slice was cut from**, not against an agent-supplied original text. The
graph already holds `(symbol_id, kind, parent_id, name, body)` for every node, so
it *is* the identity source — no re-render-with-ids needed.

### Plugin interface

`LanguagePlugin::diff(&[RenderedFile], &[RenderedFile])` is the wrong shape once
identity comes from the graph. Introduce a structural entry point, e.g.:

```rust
fn recover_operations(
    &self,
    base: &SemanticGraph,     // graph @ base_position
    scope: &[Uuid],           // the slice's root symbols (containment scope)
    edited: &[RenderedFile],  // the agent's clean text
) -> Result<Vec<Operation>>;
```

The engine owns provenance (base_position, scope) and the graph; the plugin owns
parsing + structural matching for its language. Keep the old `diff(original,
modified)` as a **legacy/offline path** (two-blob, comment-based) during migration.

## The identity-recovery algorithm

Operate per container, walking the graph's containment subtree in lockstep with
the parsed AST. For a given container (file → classes/functions; class → methods):

1. **Exact match by `(kind, name)`** against the container's existing children.
   Equal signature/body → no-op; changed body/signature → `UpdateSymbol`.
2. **Rename detection.** After exact matching, look at the leftovers: original
   children with no match, and edited nodes that matched nothing. If exactly one
   unmatched original and one new node remain in the same container, pair them as a
   rename (`UpdateSymbol` with a name change). Generalize with a body-similarity
   score (see below) when there is more than one candidate.
3. **Adds.** Edited nodes still unmatched → `CreateSymbol` with a deterministic id
   (`stable_import_uuid("method:{path}:{parent}:{name}")`, as today).
4. **Deletes.** Original children still unmatched → `DeleteSymbol`, **scoped to the
   slice** (never delete symbols in files/containers the slice did not render).
5. **References.** Re-derive call references from the edited bodies (the importer
   already does this) and diff against existing reference edges.

**Body-similarity metric.** Start simple and deterministic: normalized-line
Jaccard or token overlap between old and new body. Optionally upgrade to an
oxc-AST structural similarity later. Used only to disambiguate rename candidates;
the threshold is a tunable constant, documented and tested.

**Scope discipline.** The matcher only emits deletes/updates for symbols inside the
rendered scope. A single-file slice must never touch other files.

## Provenance & the slice lifecycle

Persist what was handed out so apply can reconstruct it:

- New `slices` table (or an `attachment` on the task/changeset):
  `(id, repository_id, branch_id, base_position, root_symbols jsonb, created_at)`.
- `slice create` writes a row and returns `slice_id`.
- `slice apply --slice-id <id>` (new) looks up `base_position` + scope,
  materializes the graph at `base_position`, and runs `recover_operations`.

**Stale base.** If the branch advanced between render and apply, the recovered
operations were computed against an older snapshot. Reuse the existing merge/rebase
machinery: treat the recovered ops as a changeset based at `base_position` and run
them through the same `analyze_merge` + replay + `tsc` path used for branch merges.
This is the one genuinely new concern statefulness introduces; it mirrors "your
branch is behind" in git and the engine already has `base_position` semantics.

## Hybrid anchor & ambiguity policy

Pure structural matching is ambiguous only when **two or more symbols in the same
container are simultaneously renamed *and* rewritten** (each looks like
delete+add). Policy, in order:

1. Resolve with body-similarity if one pairing scores clearly highest.
2. Else consult an **optional anchor**: a short, stable handle recorded in the
   slice's provenance (a per-slice `{handle -> symbol_id}` map) that the agent
   *may* preserve in a lightweight comment but is **not required** to. Used only as
   a tie-breaker, never as the primary channel.
3. Else **refuse to guess**: emit delete+create (identity lost but safe) *or*
   reject the apply with a precise diagnostic naming the ambiguous symbols. Default
   to rejection for destructive ambiguity; make it configurable.

This keeps the common case zero-token and the rare case deterministic — consistent
with the premise's "do not guess" while not taxing every edit.

## Safety nets (unchanged, still load-bearing)

- Deterministic replay + `graph.validate()` reject structurally invalid results.
- Re-render + `tsc` after apply: a mis-recovered identity that produces invalid TS
  fails loudly rather than silently corrupting (today's failure mode).
- Record the matcher's decisions (matched/renamed/added/deleted, and why) as
  changeset metadata so a reviewer can audit how text became operations.

## Phased delivery

Each phase is independently shippable and keeps the suite green.

- **P0 — oxc parsing.** Implemented. AST-based `ParsedFile`. No behavior change.
- **P1 — matcher behind the comments.** Implemented. Build `recover_operations` and prove
  equivalence: on inputs that *still have* comments, it produces the same ops as
  the comment path. Drop-in, fully tested, no rendering change yet.
- **P2 — provenance.** Implemented. Add the `slices` table + `slice create`/`slice apply
  --slice-id`; engine materializes the base graph and calls the matcher. Old
  `--original/--modified` path stays as legacy.
- **P3 — clean render.** Implemented. Stop emitting `bonhomme:symbol=` / `bonhomme:file=`
  comments from slice rendering; the matcher now relies purely on structure + base
  snapshot. Keep the header banner (human guidance, not identity).
- **P4 — stale-base handling.** Route recovered ops through the merge/rebase path
  when `base_position` < current branch length.
- **P5 — hybrid anchor + cleanup.** Add the optional anchor for ambiguous cases;
  remove dead comment-identity code; update `docs/spec-coverage.md` and tests.

## Testing strategy

- **Round-trip without metadata:** render clean → edit body → recover → expect a
  single `UpdateSymbol` on the right id; re-render + `tsc`.
- **Rename:** rename a method (clean text) → expect `UpdateSymbol{name}`, identity
  and inbound references preserved.
- **Rename + body rewrite:** the ambiguity case → assert the policy (similarity
  resolves it, or anchor resolves it, or it rejects — not a silent mis-match).
- **Add / delete within scope; no out-of-scope deletes** for a partial slice.
- **Stale base:** apply against a moved branch → routed through merge/rebase.
- **Equivalence (P1):** matcher vs comment-path produce identical ops on the
  existing comment-bearing fixtures.
- Keep determinism property tests: same edit → same operations.

## What changes / what's removed

- **Removed:** `bonhomme:symbol=` / `bonhomme:file=` from stored slice output;
  the agent obligation to preserve UUIDs; `find_symbol_id` as the *primary*
  identity source for stored slices. The legacy two-file diff still reads identity
  comments, and the duplicate-id guard still protects that path.
- **Added:** `recover_operations` (structural matcher); slice provenance
  persistence + `slice apply --slice-id`; body-similarity + ambiguity policy;
  matcher-decision audit metadata.
- **Unchanged:** the operation log as source of truth; whole-tree `import` (already
  comment-free, derives stable ids from path/name); the `validate`/`tsc` gate; the
  merge engine (reused for stale-base reconciliation).

## Open questions

- **Anchor form.** Persisted-map only, or also a tiny optional in-text handle? Lean
  persisted-map (keeps text truly clean).
- **Similarity metric.** Line/token Jaccard to start; is AST-edit-distance worth it
  later?
- **Legacy slices.** Keep the stateless `--original/--modified` path indefinitely
  (offline/no-server use) or deprecate after P3?
- **Non-method constructs.** Properties, interfaces, enums, etc. are still a
  conservative subset; the matcher must degrade gracefully on unsupported nodes
  (match what it can, leave the rest as preamble) rather than mis-attribute.
- **Reference recovery cost.** Re-deriving references from bodies on every apply vs
  diffing only changed bodies.
