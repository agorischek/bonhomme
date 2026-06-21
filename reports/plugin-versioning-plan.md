# Plan: Plugin Identity and Semantic Model Versioning

**Status:** planned. The current implementation has the beginning of this shape:
file symbols carry a `handler` metadata tag, the handler registry routes render /
recover / validate by that tag, and source snapshots have an `importer_version`.
That is enough for the prototype, but not enough once multiple handlers can model
the same language differently or once handler behavior changes over time.
**Companion reading:** [core-premise.md](core-premise.md) (operations are truth),
[fallback-handlers-plan.md](fallback-handlers-plan.md) (per-file handler routing),
[structural-identity-plan.md](structural-identity-plan.md) (graph-anchored
recovery), [config-plan.md](config-plan.md) (composition-root configuration).

## Goal

Make language/plugin identity explicit and versioned so a repository can answer:

- Which handler produced this file's semantic symbols?
- Which semantic model were those operations written against?
- Can the currently installed handler still replay, render, recover, and validate
  that model?
- Should a cache, source snapshot, or stored slice be trusted after a handler
  upgrade?

The core rule:

> A symbol tree must always be interpreted by the handler identity and semantic
> model version that created it, or by an explicit migration from that model.

Without that rule, two TypeScript handlers can both say "method" but mean different
things, and the same operation log can replay into a graph that is technically valid
but semantically wrong.

## Why this matters

Today, a handler decides what counts as a symbol. One TypeScript handler might model:

```text
file -> class -> method
```

Another might model:

```text
file -> class -> method -> local variable -> call site
```

Both are plausible, but they are not interchangeable. The model determines:

- stable symbol IDs
- parent/child containment
- conflict granularity
- reference edges
- operation recovery from slices
- rendered source fidelity
- whether old operations can still be understood

So `handler = "typescript"` is too coarse. It says which broad language family owns
the file, but not which semantic contract was used.

## Current state

- `Handler::name()` returns a short string such as `typescript`, `go`, `json`, or
  `blob`.
- File symbols store that value as metadata: `{ "handler": "typescript", "path":
  "src/app.ts" }`.
- The registry uses that tag to route render / recover / validate back to the owning
  handler.
- Source snapshots store a global `importer_version` constant, currently
  `source-snapshot-v1`.
- Graph/render cache keys are based on branch operation count and operation
  fingerprint, not renderer/plugin versions.
- Stored slices remember branch, base position, and root symbols, but not the handler
  model versions involved in recovery.

This is a good prototype shape, but it assumes handler semantics are stable forever.

## Version dimensions

Do not collapse all versioning into one number. Different changes affect different
parts of the system.

### `handler_id`

Stable identity for the handler, namespaced enough to avoid collisions.

Examples:

```text
bonhomme.typescript.oxc
bonhomme.go.go-helper
bonhomme.rust.syn
bonhomme.fallback.json
bonhomme.blob
```

This replaces the current short `handler` name as the canonical identity. The short
name can remain as a display label or legacy alias.

### `model_version`

The semantic schema emitted by the handler: symbol kinds, parentage rules, metadata
shape, ID derivation, and reference semantics.

Bump this when the operation/graph meaning changes. Examples:

- adding local variables as symbols
- changing method ID seeds
- changing whether properties are children of classes or raw file body
- changing how references are resolved
- changing metadata fields required for render/recovery

This is the most important version. A model-version mismatch can corrupt identity or
merge behavior.

### `importer_version`

The parser/import implementation version for source-to-operations. Bump this when
the same model is still used but import behavior changes.

Examples:

- bug fix in call-edge extraction
- improved support for syntax that used to degrade
- changed ignored-file rules
- parser library upgrade that affects recovered source spans

This mostly affects incremental import and source snapshots. It does not necessarily
mean old operations are unreadable.

### `renderer_version`

The graph-to-files projection version. Bump this when rendered bytes may change for
the same graph.

Examples:

- formatting change
- generated header change
- pretty-printer upgrade
- better preservation of declarations or comments

This must be part of graph/render cache invalidation. It may also affect
round-trip/session fidelity.

### `recovery_version`

The edited-slice-to-operations algorithm version. This can be folded into
`model_version` for simple handlers, but structural recovery is important enough to
track separately once it evolves.

Examples:

- rename matching threshold changes
- new move detection behavior
- broader class/type recovery support

Stored slices should know which recovery contract they were cut for.

### `validator_version`

The validation adapter contract. This is usually less important because validation
is run live, but it is still useful for diagnostics.

Examples:

- TypeScript `tsc` vs `tsgo`
- Go build command flags
- Rust workspace validation scope

External toolchain binary versions may be reported separately from the handler's own
validator adapter version.

## Target metadata

Every root file symbol should carry a full handler identity block. Descendants
inherit it through containment.

```json
{
  "handler": "typescript",
  "handlerId": "bonhomme.typescript.oxc",
  "modelVersion": "ts-symbol-model-v1",
  "importerVersion": "ts-oxc-import-v1",
  "rendererVersion": "ts-render-v1",
  "recoveryVersion": "ts-recover-v1",
  "path": "src/OrderService.ts",
  "preamble": "..."
}
```

Keep `handler` as a legacy/display alias for now. New code should prefer
`handlerId`.

For blob and fallback handlers, this matters just as much:

```json
{
  "handler": "blob",
  "handlerId": "bonhomme.blob",
  "modelVersion": "blob-file-v1",
  "importerVersion": "blob-import-v1",
  "rendererVersion": "blob-render-v1",
  "path": "logo.png"
}
```

## Trait shape

Add explicit identity to the handler boundary.

```rust
pub struct HandlerIdentity {
    pub handler_id: &'static str,
    pub display_name: &'static str,
    pub model_version: &'static str,
    pub importer_version: &'static str,
    pub renderer_version: &'static str,
    pub recovery_version: &'static str,
    pub validator_version: &'static str,
    pub legacy_names: &'static [&'static str],
}

pub trait Handler: LanguagePlugin {
    fn identity(&self) -> HandlerIdentity;
    fn claims(&self, file: &RenderedFile) -> bool;
}
```

Migration-friendly option: keep `name()` temporarily and implement it from
`identity().display_name`.

The registry should reject duplicate `handler_id`s at startup. It should also reject
duplicate legacy aliases unless an alias is explicitly marked as a compatibility
mapping.

## Routing rules

The registry should resolve a file symbol in this order:

1. `handlerId` exact match.
2. Legacy `handler` alias match.
3. Claim-by-path fallback for old untagged file symbols.
4. Terminal blob handler only if no safe owner can be found.

If a file symbol has a `handlerId` but the current registry lacks that handler, fail
with a clear compatibility error. Do not silently re-route it to another handler.

If the handler exists but does not support the file's `modelVersion`, one of two
things must happen:

- the handler contains a compatibility adapter for that model, or
- an explicit migration is run before render/recovery/write-back.

No silent reinterpretation.

## Cache keys

Graph replay only depends on operations and the core graph semantics, so the graph
portion of the cache is mostly operation-log keyed. Rendered files also depend on
the renderer versions.

Change `graph_cache` from one undifferentiated cache value to version-aware keys:

- `operation_count`
- `operation_fingerprint`
- `core_graph_version`
- `render_fingerprint`

`render_fingerprint` should be deterministic from the handler IDs and renderer
versions used by file symbols in the graph. A simple conservative first version can
use the full registry render fingerprint:

```text
bonhomme.typescript.oxc@ts-render-v1
bonhomme.go.go-helper@go-render-v1
bonhomme.blob@blob-render-v1
...
```

That may invalidate more often than strictly necessary, but it is safe.

## Source snapshots

`source_file_snapshots` should stop relying on one global importer version. Store
per-file handler identity:

```text
handler_id
model_version
importer_version
claim_version      # optional; included if claim rules become independent
renderer_version   # optional but useful for diagnostics
file_symbol_id
content_hash
last_import_position
```

Incremental import should treat a file as changed if any of these differ:

- content hash
- handler ID
- model version
- importer version
- claim version, if introduced

This prevents "unchanged bytes" from skipping re-import when the semantic model
changed underneath them.

## Stored slices

Stored slices are recovery promises. They should record enough handler identity to
detect an unsafe apply.

Add a compact recovery fingerprint to `slices`:

```json
{
  "registryFingerprint": "...",
  "handlers": [
    {
      "handlerId": "bonhomme.typescript.oxc",
      "modelVersion": "ts-symbol-model-v1",
      "recoveryVersion": "ts-recover-v1"
    }
  ]
}
```

On `slice apply --slice-id`:

1. Materialize the base graph at the stored base position.
2. Determine the file handlers involved in the slice scope.
3. Confirm the current registry can recover every recorded handler/model/recovery
   tuple.
4. If not, fail with a compatibility error instead of guessing.

This matters because a clean rendered slice has no identity comments. Recovery is
only trustworthy if the same structural model is available.

## Operation records and changesets

Do not add handler version fields to every operation as the first step. It would
bloat the log and duplicate the root file symbol metadata.

Instead:

- file `CreateSymbol` operations carry the handler identity metadata
- descendant operations inherit identity through parentage
- changesets can optionally carry a `PluginProducerAttachment` for audit/debugging

Example attachment:

```json
{
  "type": "PluginProducerAttachment",
  "handlers": [
    {
      "handlerId": "bonhomme.typescript.oxc",
      "modelVersion": "ts-symbol-model-v1",
      "importerVersion": "ts-oxc-import-v1",
      "recoveryVersion": "ts-recover-v1"
    }
  ]
}
```

If future operations can create detached symbols without a containing file, revisit
this and add explicit producer identity to those operations.

## Compatibility policy

Use these rules to decide version bumps.

### Patch handler implementation

Bug fix that does not change emitted operations, IDs, render output, or recovery
behavior:

- implementation/package version changes
- no model/importer/renderer/recovery bump required

### Importer behavior changes

Same model, but source import emits more accurate operations:

- bump `importer_version`
- source snapshots re-import changed files
- old operations remain replayable

### Render output changes

Same graph, different generated bytes:

- bump `renderer_version`
- graph/render cache invalidates
- session round-trip expectations may change

### Recovery behavior changes

Same model, different edited-slice recovery:

- bump `recovery_version`
- stored slices cut under old recovery may require old adapter or must be rejected

### Semantic model changes

Different symbol/reference schema or identity derivation:

- bump `model_version`
- provide an operation migration or maintain old-model support
- do not silently import/recover/render old files as the new model

## Migration strategy

### P0 - Document and name current models

Define stable constants for every current handler:

```text
bonhomme.typescript.oxc / ts-symbol-model-v1
bonhomme.go.go-helper / go-symbol-model-v1
bonhomme.rust.syn / rust-symbol-model-v1
bonhomme.python.treesitter / python-symbol-model-v1
bonhomme.csharp.treesitter / csharp-symbol-model-v1
bonhomme.elixir.treesitter / elixir-symbol-model-v1
bonhomme.fallback.json / json-top-level-v1
bonhomme.fallback.markdown / markdown-section-v1
bonhomme.fallback.toml / toml-span-v1
bonhomme.fallback.yaml / yaml-span-v1
bonhomme.fallback.treesitter / treesitter-structural-lite-v1
bonhomme.blob / blob-file-v1
```

The exact strings can change before implementation, but once shipped they become
compatibility contracts.

### P1 - Add `HandlerIdentity`

Add the identity struct and wire every handler with constants. Keep the old
`name()` behavior as a compatibility shim until all call sites are updated.

Add registry startup validation:

- no duplicate `handler_id`
- no accidental duplicate aliases
- terminal blob handler still exists

### P2 - Write identity metadata on import

Every handler's file `CreateSymbol` should include the full identity metadata.

Registry helper should provide a function to stamp file metadata so each plugin
does not hand-roll key names:

```rust
fn handler_metadata(identity: &HandlerIdentity, extra: serde_json::Value) -> serde_json::Value
```

Legacy graphs with only `handler` should continue to render.

### P3 - Version source snapshots

Add migration fields to `source_file_snapshots`:

- `handler_id`
- `model_version`
- `importer_version`
- optional `claim_version`

Backfill from legacy `handler` where possible. Unknown legacy values remain
renderable through alias routing but should force a full import before incremental
mode trusts them.

### P4 - Version render cache

Add a render/core fingerprint to `graph_cache`, or replace the primary cache key
with a version-aware composite. Do not serve cached rendered files if the renderer
fingerprint differs.

### P5 - Version stored slices

Add slice recovery metadata. `slice apply` should reject incompatible handler/model
versions before attempting recovery.

### P6 - Add migration hooks

Introduce an explicit path for model migrations:

```rust
trait HandlerModelMigration {
    fn from_model(&self) -> &'static str;
    fn to_model(&self) -> &'static str;
    fn migrate_operations_or_graph(&self, ...) -> Result<...>;
}
```

Do not overbuild this until there is a real model-v2 candidate. The first useful
implementation may simply be "old model is still supported, no migration needed."

## Runtime behavior

### Import

1. Registry claims each file.
2. Handler emits operations.
3. File symbols include handler identity metadata.
4. Source snapshots record handler/model/importer versions.

### Materialize/render

1. Replay operations into graph.
2. Group file symbols by `handlerId`.
3. Check each file's `modelVersion` is supported.
4. Render with the owning handler.
5. Cache using operation fingerprint plus render fingerprint.

### Recover/apply

1. Load stored slice provenance.
2. Re-materialize base graph.
3. Confirm recovery compatibility for every handler in scope.
4. Recover operations.
5. Analyze against branch drift.
6. Append only if safe.

### Validate

Validation follows the same owner routing as render. If a handler has no validator
or is a blob/fallback tier, it can return `Ok(())`, but the UI/reporting should make
that visible.

## Testing

- Registry rejects duplicate `handler_id`.
- Legacy `handler = "typescript"` file symbols still route to the TypeScript
  handler.
- A missing `handlerId` with no known alias fails clearly instead of silently
  rendering as blob.
- Incremental import treats unchanged bytes as changed when `model_version` or
  `importer_version` changes.
- Render cache misses when `renderer_version` changes.
- Stored slice apply rejects incompatible recovery/model versions.
- Mixed repo can materialize files owned by different handlers and model versions.
- Blob handler identity round-trips unchanged bytes.
- Handler breakdown reports canonical IDs, with display aliases for readability.

## Presentation phrasing

The short explanation:

> Plugins define the semantic model, so plugin identity is part of repository
> meaning. We version the model separately from importer and renderer versions
> because changing what counts as a symbol is much more serious than changing how a
> file is parsed or printed.

The stricter engineering version:

> The operation log is event-sourced, but events are only meaningful under the
> semantic model that produced them. Handler IDs and model versions are therefore
> part of the replay contract.

## Risks and open questions

- **How long do old models stay supported?** Keeping old render/recovery adapters
  forever is expensive, but forced migrations are also risky.
- **Do model versions use semver or named constants?** Named constants such as
  `ts-symbol-model-v1` are clearer for compatibility. Package semver can remain
  separate.
- **Third-party handler namespaces.** Need a convention before external plugins:
  reverse-DNS, package name, or marketplace ID.
- **Claim-rule drift.** If two handlers can claim the same file, registry order is
  part of ownership. A future `claim_version` or registry fingerprint should capture
  this.
- **Mixed-version graphs.** A long-lived repo may contain files imported under
  different model versions. The registry must route per file, not per repo.
- **Operation migrations.** Real migrations need careful auditability: they should
  themselves produce changesets or explicit migration records, not mutate history.
