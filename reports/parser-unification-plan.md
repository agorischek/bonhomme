# Plan: Parser Unification — Import as Diff-Against-Empty

**Status:** proposed (2026-06-20).
**Companion reading:** [structural-identity-plan.md](structural-identity-plan.md)
(graph-anchored recovery and clean slices), [core-premise.md](core-premise.md)
(files are projections of an operation log), [tsdoc-preservation-plan.md](tsdoc-preservation-plan.md)
(the first symptom of the problem this plan addresses).

## Problem

Each language plugin parses source **twice**, through two independent code paths
that nothing keeps in sync:

- **Read path** — `import_*` builds `Operation`s directly from a source walk
  (TS: `import.rs`, its own `with_program` call). Models the most symbol kinds.
- **Write path** — `parse_*` builds a structural model consumed by `recover/`
  (diff vs a base graph) and `diff.rs` (diff two slices). TS: `parse.rs`, a
  *separate, leaner* `with_program` walk.

A symbol kind recognized on read but not on write is **silently dropped on
edit**: editing, adding, or removing it through a slice produces no operation and
reverts on the next render. No error — the authoritative log simply never records
the change. This is not hypothetical; it is a recurring tax we keep paying:

| # | Kind dropped on the TS write path | Status |
|---|-----------------------------------|--------|
| 1 | TSDoc / godoc comments            | fixed (doc-edit recovery) |
| 2 | Class properties                  | fixed (property-edit recovery) |
| 3 | File preamble + import statements | **still live** — `recover/` and `diff.rs` contain zero `preamble`/`imports` handling |

Each fix has been a point patch threading one more field through a second parser.
The next new kind will be instance #4. The cost is structural, so the cure should
be too.

### Why Go suffers less

Go already shares **one** model: `parse_go_files` (the go-helper subprocess)
produces `Declaration`/`ParsedPackage`, and *both* `import_go_files` and
`recover_go_operations` consume it. When `doc` was added to `Declaration`, both
paths got it for free; Go's recover even diffs file metadata
(`recover_file_metadata`), which is why preamble survives there. Go unified the
**model** but not the **emission** (`operations_from_parsed_package` vs the
`recover_*` functions are still separate). TS unified neither.

## The core idea

There is exactly one engine:

```
diff(base: &SemanticGraph, model: StructuralModel) -> Vec<Operation>
```

and the three current entry points become thin wrappers:

- `import(files)`        ≡ `diff(EMPTY_GRAPH, parse(files))`   — all symbols added → Creates
- `recover(base, edited)`≡ `diff(base, parse(edited))`
- `diff_slice(a, b)`     ≡ `diff(materialize(parse(a)), parse(b))`

"Import is diff against nothing." Once import and recover are the *same code*,
they cannot disagree about which kinds exist — the drop-on-write bug class is
eliminated by construction, not by vigilance.

## The hard parts

The slogan is easy; the value is in what it forces us to unify. Each of these is
real work.

1. **One model, one extraction.** Promote the write-path model to the single
   source of truth: a *superset* covering every kind **plus file-level data**
   (preamble, imports, file symbol) with call-targets attached. Rewrite `import`
   to emit from this model instead of its own walk. (TS: collapse the two
   `with_program` walks into one.)

2. **One identity resolver.** There are currently **three** strategies:
   - import *derives* ids from names: `stable_import_uuid("class:{path}:{name}")` etc.
   - recover *looks up* base ids in the graph.
   - `diff_slice` reads `bonhomme:symbol=` *comments, then falls back to name*.

   The engine needs a single resolver: given a parsed symbol, the base graph, and
   an optional embedded id, return the stable id. This is the crux — wrong here
   means dangling references or duplicated symbols.

3. **One matcher.** Recover detects renames by body similarity
   (`recover/matcher.rs`, the `SymbolLike`/`EditedLike` traits); import has no
   matcher (all new); properties match by name (no body). The diff needs one
   match step that is a no-op against an empty base and handles body-bearing
   kinds (similarity) and bodyless kinds (name) uniformly.

4. **References as a second diff.** Symbol diffing must finish first — a call's
   target id is unknown until the callee is placed — then the reference set is
   diffed (Create/DeleteReference). Import = all references created. Today
   `import_references` and `recover_reference_operations` are separate
   implementations; they become one.

5. **File-level into the model.** Preamble, imports, and the file symbol become
   model fields diffed like everything else. **This is where instance #3
   (preamble drop) dies for free**, as a property of the architecture rather than
   a patch.

6. **Metadata always rebuilt from the model.** `UpdateSymbol.metadata` *replaces*
   the whole blob (the original cause of the doc-drop). The unified Update path
   reconstructs the full metadata from the model every time — no hand-written
   partial arm survives that can "forget a field." This structurally kills the
   doc-drop class.

7. **One deterministic ordering** satisfying `materialize`'s invariants: parents
   before children on create; references after symbols; deletes ordered so a
   reference is removed before either endpoint.

## Phases

**P0 — Characterization harness (do this first; low risk; independently useful).**
Pin the exact `Operation` stream that `import`, `recover`, and `diff_slice`
produce for a corpus of fixtures (classes, methods, properties, docs, preamble,
references, renames, edge cases). Golden assertions. The refactor's correctness
criterion is then **byte-identical op-stream** — any divergence in ids, ordering,
or metadata fails loudly. Without this net, this refactor is how subtle op-stream
corruption reaches the authoritative log. Build the harness; treat the goldens as
the spec.

**P1 — One model.** Extend the write-path model to the superset (all kinds +
file-level data + references). Land it behind the existing `parse_*`; no behavior
change yet. Add the parity guardrail (a test asserting import and parse see the
same `(kind, name)` set for a fixture) as a stopgap that survives into P2.

**P2 — Import emits from the model.** Rewrite `import` as "create everything in
the model." Goldens from P0 must stay byte-identical. This removes the second
extraction; import and parse now share one walk.

**P3 — One identity resolver + matcher.** Extract identity resolution and the
rename matcher into a single component used by import (trivial / empty base),
recover, and diff. Goldens hold.

**P4 — Unify symbol diff.** Introduce `diff_symbols(base, model)`; express
`recover` and `diff_slice` on it; `import = diff_symbols(EMPTY, model)`. Goldens
hold.

**P5 — Unify references + file-level.** Move reference diffing and file-level
(preamble/imports) diffing into the engine. **Instance #3 is fixed here**; add
its regression test. Collapse `import`/`recover`/`diff_slice` into thin wrappers.

**P6 — (Ambitious, optional) Core-generic engine.** Lift `diff(base, model)` into
`bonhomme-core` over a `StructuralModel` trait so every plugin gets
import+recover+diff from one extractor. Highest payoff (new plugins are
divergence-proof by default) but the biggest lift, and it touches the plugin
boundary the parallel team is actively reshaping — schedule against their churn.

Per-language scope: do TS through P5 first (it is the outlier and the proving
ground), then bring Go's emission onto the shared engine, then weigh P6.

## Risks & mitigations

- **Op-stream regressions in load-bearing code.** `import.rs` is the most-tested,
  most-depended-on code; a subtle id/ordering/metadata change can break
  `materialize` or downstream consumers silently. → P0 goldens are the gate;
  nothing merges that changes the stream for unchanged input.
- **Merge collision with the parallel team.** They are reshaping the core/plugin
  boundary (`bonhomme-markdown`, lang registry, `parse.go`). → Keep P1–P5
  TS-internal; defer P6 (the core-boundary change) until their churn settles;
  isolate commits per the standing constraint
  ([[isolate-commits-from-team-edits]]).
- **Hidden coupling in identity.** The three id strategies may encode
  intentional differences (e.g. comment-anchored ids in slices vs name-derived in
  import). → Enumerate them in P3 with a test per strategy before collapsing.

## Success criteria

- `import`, `recover`, and `diff_slice` are wrappers over one `diff(base, model)`.
- One place recognizes symbol kinds; adding a kind cannot diverge read vs write.
- A documented, propertied, import-bearing file survives the full lifecycle
  (import → render → **edit any element** → recover/diff) with zero silent drops.
- P0 goldens are byte-identical across the whole refactor for unchanged input.

## Out of scope / open questions

- Whether `diff_slice`'s `bonhomme:symbol=` comment anchoring stays a distinct
  identity source or is subsumed by the resolver (P3 decides).
- Whether P6's `StructuralModel` trait lives in `bonhomme-core` or a new
  `bonhomme-structural` crate.
- Interaction with merge/conflict analysis, which also consumes operation
  streams — out of scope here, but the unified engine should make it simpler.
