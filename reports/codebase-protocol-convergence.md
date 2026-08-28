# Report: bonhomme and Codebase Protocol Convergence

> **Status:** exploratory report (2026-06-23).
> **Companion reading:** [core-premise.md](core-premise.md) (operation log as
> authority; files as projections), [parser-unification-plan.md](parser-unification-plan.md)
> (import/recover/diff convergence), [plugin-versioning-plan.md](plugin-versioning-plan.md)
> (model and handler versions), [config-plan.md](config-plan.md) (configuration
> spine).

## Summary

bonhomme and Codebase Protocol are moving toward the same premise:

> A codebase is a database, not a directory tree.

The non-convergence is illuminating because the two projects approach that
database from opposite sides.

- **Codebase Protocol has the stronger read/execution model.** It names the
  objects external tools want: snapshots, projects, modules, artifacts,
  contexts, relationships, queries, semantic resolution, and tool execution.
- **bonhomme has the stronger write/history model.** It has immutable semantic
  operations, branches, changesets, graph replay, semantic merge analysis,
  conflict detection, language plugins, and filesystem projections for
  compatibility.

The likely convergence point is not "make bonhomme expose the current API under
CBP names." It is a core-model shift:

```text
Codebase
  -> transactions / changesets
  -> immutable snapshots
  -> entity-fact-relationship graph
  -> context resolution
  -> tool execution
  -> diagnostics and artifacts
  -> optional filesystem projection
```

bonhomme should keep its operation log and merge semantics, but reify the
materialized graph as a snapshot-addressed entity/fact/relationship database.
CBP should keep its minimal protocol posture, but absorb a real
transaction/history model and soften its identity rule so stable semantic
entities can survive rename, move, and body changes.

## The useful disagreement

The disagreement is not mostly about implementation. It is about which questions
the core model answers first.

CBP asks:

- What does a tool need to read?
- How does a tool resolve semantic inputs without walking a filesystem?
- How does execution become stateless and snapshot-addressed?
- How do configuration and module resolution become codebase-owned data?

bonhomme asks:

- What does an edit mean?
- How can many agents edit concurrently without line-merge noise?
- How can the system replay, validate, review, and merge semantic changes?
- How does ordinary source text remain usable while no longer being the source
  of truth?

Those are both database questions. The combined model needs both faces:

- a **queryable database state** for tools;
- a **transactional history** for edits, review, merge, and conflict handling.

## Current bonhomme model

bonhomme's source of truth is an operation log. The materialized semantic graph
and rendered source files are reconstructed from immutable operations.

Core concepts today:

- `Repository`
- `Branch`
- `Task`
- `ChangeSet`
- `OperationRecord`
- `SymbolNode`
- `ReferenceNode`
- `SemanticGraph`
- `RenderedFile`
- `Slice`

The operation vocabulary is compact:

```text
CreateSymbol
DeleteSymbol
UpdateSymbol
MoveSymbol
CreateReference
DeleteReference
```

That is a good write model. It is small enough for replay, review, and semantic
merge. Its limitation is that it treats the materialized graph as symbols and
references rather than as a broader database of projects, modules, contexts,
artifacts, diagnostics, and relationships.

bonhomme also still exposes file-shaped compatibility objects:

```text
RenderedFile {
  path,
  content
}
```

That is useful and should not disappear. Existing compilers, editors, formatters,
and agents still speak source text. But for CBP convergence, file paths should be
projection metadata, not core identity.

## Current CBP model

CBP's core concepts are:

- `Codebase`
- `Snapshot`
- `Project`
- `Module`
- `Artifact`
- `Context`
- first-class relationships

Its primitive operations are:

```text
query(...)
resolve(...)
mutate(...)
snapshot(...)
```

and tool execution is expressed against a snapshot:

```text
execute(snapshot, tool, args)
```

That is a good read/execution model. Its limitation is that the write side is
too thin. `mutate(...) -> newSnapshotId` says the right thing at a high level,
but it does not yet model:

- transactions;
- changesets;
- authorship;
- branch/ref movement;
- operation provenance;
- merge;
- semantic conflicts;
- recovery from edited projections;
- reviewable intent.

CBP also states that all identities are hash-derived. That is right for
immutable values and snapshots, but wrong for stable semantic entities. A method
or module may be renamed or moved while remaining the same logical thing.

## Non-convergence map

| Area | CBP bias | bonhomme bias | Convergence |
|---|---|---|---|
| State identity | Immutable `SnapshotId` | Branch plus operation position | First-class snapshots derived from operation/fact roots |
| Change identity | Under-specified mutation | Operation records and changesets | Transactions that produce snapshots |
| Entity identity | Hash-derived | Stable UUID-like symbol IDs | Stable `EntityId`; hash IDs for immutable values |
| Code shape | Projects/modules/artifacts/contexts | Symbols/references/files | Generic entity/fact/relationship graph |
| Relationships | First-class | Containment partly embedded in symbols | Relationship rows for all edges |
| Files | Explicit non-goal | Compatibility projection | Projection locators outside semantic identity |
| Config | First-class `Context` | Composition-root config and validator inputs | Context entities resolved by snapshot/tool/target |
| Tool outputs | Diagnostics/mutations/artifacts | Validation errors and rendered files | Durable artifact and diagnostic records |
| Execution | Stateless against snapshot | Local toolchain over rendered temp tree | Hidden runner now; CBP-native executor later |
| Merge | Not modeled | Semantic operation merge | CBP transaction and conflict model |

## Proposed shared core model

The shared model should be a durable semantic database with four identity
families:

```text
SnapshotId
  hash-derived immutable database state

ContentId
  hash-derived immutable value: source body, metadata blob, config object,
  artifact bytes, diagnostic report

EntityId
  stable logical identity: project, module, symbol, context, artifact handle

TransactionId
  stable identity for a committed change event
```

The most important split is `EntityId` versus `ContentId`.

CBP should not require every semantic entity to be hash-derived. Immutable values
should be content-addressed. Immutable snapshots should be content-addressed.
Stable semantic entities should survive content changes.

### Codebase

```text
Codebase {
  id: CodebaseId
  name
}
```

This maps directly to bonhomme's `Repository`, though `Codebase` is the better
protocol name.

### Ref

```text
Ref {
  codebase_id
  name
  target_snapshot_id
}
```

A branch should become a mutable ref to an immutable snapshot. This preserves
bonhomme's branch ergonomics while giving CBP a clean snapshot execution target.

### Snapshot

```text
Snapshot {
  id: SnapshotId
  codebase_id
  parent_snapshot_ids: Vec<SnapshotId>
  transaction_id: Option<TransactionId>
  model_version
  fact_root_hash
}
```

Snapshots are immutable. A branch/ref can move. A snapshot cannot.

bonhomme can derive snapshots from operation replay:

```text
operations visible at ref
  -> materialized fact database
  -> canonical serialization
  -> SnapshotId
```

The operation log remains authoritative. The snapshot is the immutable state
object used by external tools.

### Transaction

```text
Transaction {
  id: TransactionId
  codebase_id
  base_snapshot_id
  result_snapshot_id
  changeset_id
  author
  created_at
  operations: Vec<Operation>
}
```

CBP's `mutate(...)` should create a transaction. A transaction produces a
snapshot. This keeps mutation stateless while giving humans and agents something
reviewable.

### Entity

```text
Entity {
  id: EntityId
  kind: project | module | symbol | context | artifact | diagnostic | ...
  created_in: TransactionId
  retired_in: Option<TransactionId>
}
```

`SymbolNode` becomes one kind of entity, not the core abstraction. This lets
projects, modules, contexts, artifacts, diagnostics, and future concepts live in
the same graph.

### Fact

```text
Fact {
  entity_id: EntityId
  key: String
  value: ContentId | scalar
}
```

Facts replace fixed columns like `kind`, `name`, `body`, and `metadata` as the
long-term storage shape. The current `SymbolNode`:

```text
SymbolNode {
  id
  parent_id
  kind
  name
  body
  metadata
}
```

can be represented as:

```text
Entity(symbol_id, kind = "symbol")
Fact(symbol_id, "symbol.kind", "method")
Fact(symbol_id, "name", "displayName")
Fact(symbol_id, "body", content_id)
Fact(symbol_id, "metadata", content_id)
Relationship("contains", parent_id, symbol_id)
```

The fixed struct can remain as an optimized view while the protocol model moves
toward facts.

### Relationship

```text
Relationship {
  id: RelationshipId
  kind: String
  source_entity_id: EntityId
  target_entity_id: EntityId
  metadata: ContentId | object
}
```

This absorbs both CBP's relationship vocabulary and bonhomme's references.

Examples:

```text
PROJECT_CONTAINS_MODULE
MODULE_CONTAINS_SYMBOL
SYMBOL_CONTAINS_SYMBOL
MODULE_IMPORTS_MODULE
SYMBOL_CALLS_SYMBOL
CONTEXT_APPLIES_TO_MODULE
ARTIFACT_GENERATED_FROM_MODULE
DIAGNOSTIC_APPLIES_TO_SYMBOL
```

Containment should become a relationship, not a `parent_id` field.

### Projection locator

Files should become projection metadata:

```text
ProjectionLocator {
  entity_id
  projection_kind: filesystem
  locator: "src/OrderService.ts"
}
```

CBP can continue to reject files and paths at the protocol level. bonhomme can
still render files for compatibility. The important rule is:

> Paths may locate projections. Paths must not be semantic identity.

### Context

CBP is ahead of bonhomme here. Context should be a first-class entity:

```text
Context {
  id: EntityId
  tool: "typescript" | "prettier" | "eslint" | "go" | "cargo"
  target_entity_id
  options_content_id
  resolved_from_snapshot_id
}
```

Then tool configuration becomes queryable database state:

```text
resolve({
  kind: "context",
  tool: "typescript",
  target: module_id,
  snapshot: snapshot_id
})
```

This is a cleaner long-term home for data currently split between
`bonhomme.toml`, language plugins, validators, and rendered workspace files.

### Artifact

Artifacts should become durable database objects:

```text
Artifact {
  id: EntityId
  artifact_kind: rendered_source | declaration | bundle | test_result |
                 diagnostic_report | object_file
  content_id
  generated_from_snapshot_id
  generated_by_tool
  context_id
}
```

Rendered files can be artifacts. Diagnostics can be artifacts. Build outputs can
be artifacts. This is where CBP's model should inform bonhomme strongly.

## Changes needed in bonhomme

### B1. Add first-class snapshots

bonhomme should stop treating "branch plus operation count" as the only durable
way to name state. Keep branches, but make them refs to snapshots.

Initial implementation can be thin:

```text
snapshot_id = hash(canonical operation ids + operation payloads + model versions)
```

Later implementation should hash the canonical materialized fact database.

Acceptance criteria:

- every materialized branch response includes a `snapshotId`;
- a snapshot can be materialized without naming a branch;
- cache keys use `snapshotId`, not only branch id and operation count;
- snapshots remain valid after refs move.

### B2. Split stable entity identity from content identity

Do not replace stable symbol IDs with content hashes. Instead, introduce content
IDs for immutable values:

```text
body_content_id
metadata_content_id
artifact_content_id
context_options_content_id
```

This lets a symbol's body change without changing the symbol's identity, while
still making immutable values content-addressed.

This also gives a better answer for large and binary content than inlining bytes
or base64 into source-shaped records.

### B3. Generalize symbols/references toward entities/facts/relationships

The current structs are useful:

```text
SymbolNode
ReferenceNode
SemanticGraph
```

But the core database should grow a more general layer under or beside them:

```text
Entity
Fact
Relationship
```

The current graph can become a typed view over those rows. This avoids forcing
every future concept into "symbol" or "reference."

The first migration does not need to rewrite all storage. A practical sequence:

1. Add protocol-level structs in `bonhomme-core`.
2. Derive them from `SemanticGraph`.
3. Serve CBP-style queries from the derived model.
4. Persist facts/relationships once the shape proves itself.

### B4. Promote module and project entities

Today, file symbols often stand in for modules. That is a compatibility
shortcut, not a database model.

Add first-class entities:

```text
Project
Module
Package
CompilationUnit, if a language needs it
```

Then decide per language how these map:

- TypeScript: module may be a source module, package, or tsconfig project.
- Go: project may be a module; package is a module-like entity.
- Rust: project may be a workspace or crate; modules are crate modules.
- C#: project may be a `.csproj`; modules may be compilation units/namespaces.

The exact mapping can be language-specific. The existence of a module entity
should not be.

### B5. Make context resolution a core service

Add a core operation/query shape for contexts before making tool execution fancy.

Example:

```text
resolve_context(snapshot_id, tool, target_entity_id) -> Context
```

Initial contexts can be produced from existing config and plugin defaults. Over
time, language plugins can import config files as structured context facts.

This creates a path from today's validators to CBP's rule:

> Tools never discover configuration themselves. They request a context.

### B6. Make artifacts and diagnostics durable

Rendered files are currently compatibility outputs. Treat them as artifact
records:

```text
render(snapshot_id) -> Vec<Artifact>
validate(snapshot_id, context_id) -> DiagnosticArtifact
```

Do not require every artifact to be stored forever. But the model should support
durable artifact identity, content addressing, provenance, and query.

### B7. Keep filesystem projection as an adapter

bonhomme should continue to render files. That is the bridge to current tools and
agents. The change is to make the boundary explicit:

```text
semantic database
  -> projection artifact
  -> filesystem materialization, if needed
```

This lets CBP-native tools avoid paths while legacy tools still work.

## Changes needed in CBP

### C1. Add transactions

`mutate(...) -> newSnapshotId` is not enough for review, provenance, merge, or
conflict handling.

CBP should return:

```text
MutationResult {
  transaction_id
  snapshot_id
  diagnostics
  conflicts
}
```

The transaction is the durable write event. The snapshot is the durable resulting
state.

### C2. Add refs without making execution stateful

CBP execution should remain snapshot-addressed:

```text
execute(snapshot_id, tool, args)
```

But users and agents need mutable names:

```text
main
agent-017
release/1.2
```

CBP should model refs as convenience names that resolve to snapshots. The
execution primitive should not depend on refs.

### C3. Distinguish stable entities from immutable values

Revise the identity rule:

> Immutable values, artifacts, and snapshots are content-addressed. Stable
> semantic entities have durable logical identities.

This preserves the good part of content addressability without destroying
semantic identity.

### C4. Define conflict output

CBP should not prescribe bonhomme's exact merge algorithm, but it should have a
semantic conflict shape.

Example:

```text
Conflict {
  kind
  source_entity_id
  target_entity_id
  relationship_id
  base_snapshot_id
  current_snapshot_id
  proposed_transaction_id
  message
}
```

This makes conflicts queryable and reviewable rather than implementation
failures.

### C5. Clarify modules versus projections

CBP rejects paths, but it should still explain how path-based ecosystems are
projected.

Suggested rule:

> A module is a semantic codebase entity. A filesystem path is one possible
> projection locator for a module or artifact.

This gives implementations permission to interoperate with existing compilers
without making paths protocol concepts.

## Query model implications

CBP's `query(...)` can map naturally to the entity/fact/relationship model.

Examples:

```text
query({
  snapshot,
  kind: "entities",
  entityKind: "module",
  language: "typescript"
})
```

```text
query({
  snapshot,
  kind: "relationships",
  relationshipKind: "MODULE_IMPORTS_MODULE",
  source: module_id
})
```

```text
query({
  snapshot,
  kind: "facts",
  entity: symbol_id
})
```

bonhomme's existing symbol and relationship queries can become compatibility
helpers over this more general query surface.

## Mutation model implications

bonhomme operations are already close to database writes. The generalized
operation vocabulary would look like:

```text
CreateEntity
RetireEntity
SetFact
UnsetFact
CreateRelationship
DeleteRelationship
```

The existing operation vocabulary maps cleanly:

```text
CreateSymbol      -> CreateEntity + SetFact* + CreateRelationship(contains)
UpdateSymbol      -> SetFact*
MoveSymbol        -> DeleteRelationship(contains) + CreateRelationship(contains)
DeleteSymbol      -> RetireEntity
CreateReference   -> CreateRelationship
DeleteReference   -> DeleteRelationship
```

This is a natural long-term core. The current symbol operations can remain as a
typed facade until the generalized operations are worth the migration cost.

## Tool execution implications

There are two viable stages.

### Stage 1: CBP facade over compatibility projection

The server accepts CBP-style requests:

```text
execute(snapshot, "typescript", args)
```

Internally it:

1. materializes the snapshot;
2. renders projection artifacts;
3. writes a hidden temporary workspace if the legacy tool requires it;
4. runs the tool;
5. imports diagnostics/artifacts/mutations back into the database model.

This is not pure serverless CBP, but it lets bonhomme expose the protocol early.
The filesystem remains hidden implementation detail.

### Stage 2: CBP-native execution

Tool adapters consume modules, contexts, artifacts, and content blobs directly.

Possible implementation strategies:

- in-memory virtual filesystem;
- WASM-hosted toolchains;
- remote execution workers;
- object-storage-backed materialization;
- tool-specific protocol adapters.

Stage 2 is expensive. The core model should not wait for it.

## Migration sequence

### P0: Naming and protocol structs

Add Rust structs for:

- `CodebaseId`
- `SnapshotId`
- `EntityId`
- `ContentId`
- `TransactionId`
- `Entity`
- `Fact`
- `Relationship`
- `Context`
- `Artifact`

Do not rewire storage yet.

### P1: Snapshot IDs for materialized branches

Compute and return `snapshotId` from materialization. Keep branches as they are.

This gives external callers a stable state name and starts separating "what
state is this?" from "which branch did I ask for?"

### P2: Derived entity/fact/relationship view

Derive a generic database view from `SemanticGraph`.

This should be read-only at first. It proves query shape without risking the
operation log.

### P3: CBP-style query and resolve API

Implement:

```text
query(snapshot, ...)
resolve(snapshot, ...)
```

Back the first version with the derived view.

### P4: Context entities

Represent tool contexts as database entities. Initial contexts can be derived
from existing config, plugin defaults, and imported config files.

### P5: Artifact records

Represent rendered files, validation outputs, and diagnostic reports as
artifacts with content IDs and provenance.

### P6: Generalized operations

Introduce generic fact/relationship operations underneath or alongside the
current symbol operations. Migrate one language path only after the derived view
has stabilized.

## Design tests

The convergence model should pass these conceptual tests.

### Rename preserves identity

Renaming a method changes facts, not entity identity:

```text
EntityId(method) stays the same
Fact(name) changes
SnapshotId changes
```

### Move preserves identity

Moving a class between modules changes containment relationships, not entity
identity:

```text
DeleteRelationship(old_module CONTAINS class)
CreateRelationship(new_module CONTAINS class)
```

### Tool execution is reproducible

Given the same:

```text
snapshot_id
tool
context_id
arguments
tool_adapter_version
```

the result should not depend on request history or cache presence.

### Files are optional

A legacy compiler may need filesystem materialization. A CBP-native analyzer
should be able to query modules and contexts without ever seeing paths.

### Merge conflicts are semantic

Two transactions creating methods with the same name under the same parent should
conflict because of a duplicate semantic relationship/fact constraint, not
because of neighboring source lines.

## Open questions

### Is `Module` language-neutral enough?

CBP uses `Module` as a primary concept, but languages disagree:

- TypeScript module
- Go package
- Rust module/crate
- C# compilation unit/namespace/project
- Elixir module

The protocol may need both `Module` and language-specific module subkinds. The
core should not assume that a module equals a file.

### Should facts be fully generic in storage?

A generic fact table is elegant and protocol-friendly, but typed structs are
faster and easier to reason about. bonhomme may want:

- typed views for hot paths;
- generic facts for protocol/query/artifact/context extensibility.

The first implementation should derive facts from typed structs rather than
immediately replacing them.

### What is the canonical snapshot hash?

Options:

1. hash canonical operation records;
2. hash the materialized entity/fact/relationship database;
3. hash both, with one as provenance and one as state identity.

For CBP, state identity should eventually be the materialized database hash. For
bonhomme migration, operation hash is easier and still useful.

### Are contexts imported source or resolved views?

Configuration files can be ordinary modules/artifacts, but a resolved context is
a computed object:

```text
config source entities + language rules + target module -> resolved context
```

The model should probably store both.

## Recommended direction

The best convergence path is:

1. Keep bonhomme's operation log as the authoritative write history.
2. Add first-class immutable snapshots as the read/execution unit.
3. Generalize the materialized graph into entities, facts, and relationships.
4. Treat projects, modules, contexts, artifacts, and diagnostics as first-class
   entities.
5. Keep filesystem rendering as a projection artifact, not as semantic identity.
6. Adjust CBP so transactions, refs, stable entity IDs, and conflicts are part
   of the model.

In short:

> bonhomme should grow CBP's database-shaped read surface.
> CBP should grow bonhomme's transaction-shaped write surface.

That combined model is the strongest version of "a codebase is a database":
queryable like a database, reproducible like content-addressed storage, editable
like a transaction log, and still projectable into files when the existing tool
world needs them.
