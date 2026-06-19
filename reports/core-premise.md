# bonhomme Core Premise

## Summary

bonhomme is an experimental source control system for codebases edited by both humans and many AI agents.

The core idea is simple:

> Store the meaning of code changes, not just the final text of files.

Git stores file snapshots and computes differences between lines of text. That model is powerful, durable, and broadly useful, but it was designed around human-scale editing: a small number of people making relatively coherent changes over time.

bonhomme starts from a different assumption: future software projects may have dozens or hundreds of agents editing the same codebase at once. If every agent edits files directly, line-based merging quickly becomes noisy. Two agents can safely add different methods to the same class, but Git still sees both agents editing the same file, sometimes the same nearby region of text.

bonhomme tries to move the source of truth up one level. Instead of treating files as the canonical repository state, it treats semantic operations as canonical.

Files still exist. TypeScript still compiles. Editors and build tools still work. But in bonhomme, files are projections rendered from a semantic operation log.

## The Central Shift

In a normal Git workflow, the repository is mostly understood as a tree of files:

```text
Repository
  -> Files
  -> Text diffs
  -> Merge result
```

In bonhomme, the repository is understood as a history of semantic operations:

```text
Intent
  -> Task
  -> ChangeSet
  -> Operations
  -> Semantic Graph
  -> Rendered TypeScript Files
```

The operation log is authoritative.

The semantic graph is a materialized view.

The files are compatibility output.

This distinction matters. If the operation log is the source of truth, then repository state can always be reconstructed by replaying operations in order. A rendered file is not the thing that must be preserved forever. It is the current TypeScript-facing view of the graph.

## What An Operation Is

An operation is an immutable semantic mutation.

Examples:

```json
{
  "type": "CreateSymbol",
  "symbolId": "7b2f...",
  "parentId": "91aa...",
  "kind": "method",
  "name": "displayName",
  "body": "return \"OrderService\";",
  "metadata": {
    "signature": "displayName(): string"
  }
}
```

Another example:

```json
{
  "type": "CreateReference",
  "referenceId": "c139...",
  "fromSymbolId": "7b2f...",
  "toSymbolId": "0a12...",
  "kind": "calls"
}
```

The important property is that these operations are not textual patches. They say what changed in the code model:

- A symbol was created.
- A symbol was renamed or updated.
- A symbol was deleted.
- A reference was created.
- A reference was deleted.

Version 1 keeps the core model deliberately small. The core system understands only symbols, references, containment, and reference edges. Richer concepts like classes, methods, interfaces, decorators, and modules are treated as plugin-level concerns.

## Why Symbols Matter

A symbol is a stable identity for a piece of code.

Names can change. Files can move. A method can be renamed. A class can be reorganized. But the symbol ID remains the same.

That gives bonhomme a way to separate identity from presentation.

For example, suppose an agent renames a method:

```text
displayName -> serviceLabel
```

In Git, this is a text edit.

In bonhomme, it can be represented as:

```text
UpdateSymbol(symbolId=..., name="serviceLabel")
```

That means review tools can show the semantic change directly: "method renamed," not merely "this line was deleted and this other line was added."

## Why This Helps Many Agents

The system is built for a world where agents do not directly own files. They own slices.

A slice is an editable projection of part of the repository:

```text
Slice
  -> requested symbols
  -> dependency context
  -> public surfaces of nearby code
  -> recent related changes
```

An agent receives a slice as TypeScript text because text is still the natural interface for coding tools. The agent edits that slice. Then bonhomme compares the original slice with the modified slice and turns the difference back into operations.

The agent does not submit a file patch. It submits semantic operations derived from its edit.

This changes the merge problem.

If two agents both edit the same class:

```text
agent-a: CreateSymbol(method, "carrierRoutingPlan")
agent-b: CreateSymbol(method, "refundReadiness")
```

Those are independent operations. They can merge safely even though both would normally touch the same TypeScript file.

If two agents both create a method with the same name under the same parent:

```text
agent-a: CreateSymbol(method, "audit")
agent-b: CreateSymbol(method, "audit")
```

That is a semantic conflict. bonhomme should not guess. It should report `CONFLICT`.

## The Graph Is A View, Not The Source Of Truth

bonhomme materializes a semantic graph by replaying operations.

The graph contains:

- Symbol nodes
- Reference nodes
- Contains relationships
- References relationships

The graph is useful for queries, rendering, review, and validation. But it is not authoritative.

This is intentional. A cached graph can be discarded. A rendered file can be regenerated. The operation log is what must survive.

That means a repository state is reproducible:

```text
Operation log
  -> replay
  -> semantic graph
  -> rendered files
  -> TypeScript compiler validation
```

If replaying the same operation log produces different files, that is a system bug.

## Files Still Matter

bonhomme is not trying to replace the TypeScript compiler, editor, package manager, test runner, or build system.

The system still renders TypeScript files because existing tooling expects files.

A rendered file may include hidden metadata comments:

```ts
export class OrderService /* bonhomme:symbol=6343... */ {
  displayName(): string /* bonhomme:symbol=63ba... */ {
    return "OrderService";
  }
}
```

Those comments preserve identity across text editing without affecting TypeScript compilation.

This is the compatibility bridge:

```text
Semantic operation log
  -> graph
  -> TypeScript files
  -> tsc / tests / editor tooling
```

## Review Changesets, Not Line Diffs

In bonhomme, a ChangeSet is the unit of review.

A ChangeSet groups operations produced by a task or agent. Instead of reviewing only a line-based diff, a reviewer can inspect the semantic changes:

```text
ChangeSet: agent-017 delivery promise

Operations:
  - CreateSymbol(method, "deliveryPromise")
  - CreateReference(calls, from deliveryPromise to displayName)
  - CreateReference(calls, from deliveryPromise to listOrders)
```

This is meant to make review more legible when there are many concurrent agents. A reviewer can ask:

- What symbols did this task create?
- What existing symbols did it update?
- What dependencies did it introduce?
- What references did it create or delete?
- Does this conflict semantically with another ChangeSet?

Text diffs are still useful, but they are no longer the only review surface.

## Merge Philosophy

bonhomme merges operations.

It does not merge files.

It does not merge text.

Version 1 has only two merge outcomes:

```text
SAFE_MERGE
CONFLICT
```

The system should not silently rewrite, guess intent, or use AI to invent a resolution.

That restraint is part of the design. AI agents can generate changes, but the source control system itself should remain deterministic and auditable.

## Validation

Every operation application must preserve graph invariants.

Examples:

- No duplicate symbol IDs
- No dangling parent links
- No dangling references
- No duplicate sibling symbols
- All references resolve

After merge, bonhomme renders TypeScript and runs `tsc`. The TypeScript compiler acts as an external validator for the rendered projection.

The validation chain is:

```text
Operation
  -> graph invariant checks
  -> deterministic render
  -> TypeScript compiler
```

## What The Current Prototype Implements

The current bonhomme prototype implements the core shape of the system:

- Rust CLI and API
- PostgreSQL operation-log storage
- Repositories, branches, tasks, changesets, operations, and attachments
- Immutable semantic operations
- Deterministic graph replay
- Graph validation
- TypeScript rendering with hidden symbol metadata
- Conservative TypeScript import for common constructs
- Slice diff for method edits, top-level function edits, deletes, and new files
- Operation-level merge with `SAFE_MERGE` or `CONFLICT`
- TypeScript compiler validation after merge
- Query commands for symbols, references, callers, callees, dependencies, and dependents
- Persistent graph/render cache derived from operation replay
- React/Vite demo for many simultaneous agent branches
- Simulation command/API for deterministic multi-agent merge runs

This is enough to demonstrate the premise:

```text
Many branches can independently add semantic operations.
Independent additions merge cleanly.
The repository can be reconstructed from the operation log.
Rendered TypeScript can compile.
The review surface can show operations instead of only line diffs.
```

## What The Prototype Does Not Yet Prove

The prototype does not yet prove that bonhomme can faithfully model all TypeScript.

Important missing pieces include:

- A full TypeScript AST and type-checker-backed plugin
- Complete import fidelity for arbitrary TypeScript projects
- Semantic diff for class edits, interfaces, enums, namespaces, decorators, and complex reference changes
- File delete handling
- Arbitrary branch DAG merges
- Rich semantic review UI
- Large-scale performance tuning beyond initial simulations

Those are not small details. They are the hard parts between a promising prototype and a serious source control system.

## The Bet

The bet behind bonhomme is that agent-heavy software development needs a source control model with a better unit of change than "edited lines in files."

Files are still necessary, but they are not always the best source of truth.

If the system can reliably convert between:

```text
semantic operations <-> graph <-> TypeScript files
```

then merging, review, provenance, and reconstruction can all become more precise.

bonhomme is the first cut at that idea.
