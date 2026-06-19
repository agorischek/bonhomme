# Spec Coverage

bonhomme currently implements a runnable v1 prototype, not a complete production implementation of the draft.

## Implemented

- Rust CLI/API named `bonhomme`
- PostgreSQL operation-log storage
- repositories, branches, tasks, changesets, operations, attachments
- immutable operations: `CreateSymbol`, `DeleteSymbol`, `UpdateSymbol`, `CreateReference`, `DeleteReference`
- deterministic graph replay from operation records
- validation for duplicate IDs, dangling parents, dangling references, and duplicate sibling symbols
- TypeScript import for conservative `.ts` subsets: files, classes, methods, properties, top-level functions, and call references
- TypeScript slice rendering with hidden symbol metadata in comments
- slice apply for method additions/updates/deletes, top-level function additions/updates/deletes, and new TypeScript files in rendered slices
- operation-level merge with deterministic `SAFE_MERGE` or `CONFLICT`
- TypeScript compiler validation after merge and during `validate`
- persistent graph/render cache keyed by branch operation count and operation-id fingerprint
- queries: find symbol, references, callers, callees, dependencies, dependents
- React/Vite visual demo for many agent branches and semantic merge review
- deterministic simulation command/API for many agent branches, final replay/render checks, and compiler validation
- unit tests, a property-style merge commutativity test, a 512-agent in-memory simulation test, and importer round-trip coverage

## Still Incomplete

- Full TypeScript AST and type-checker-backed semantic model
- General import fidelity for every TypeScript construct
- Semantic diff for class edits, file deletes, reference updates, properties, interfaces, enums, decorators, and namespaces
- Arbitrary branch DAG merge support beyond the current direct branch-to-target workflow
- Broader randomized simulation suite across destructive edits, deletes, updates, and non-method symbols
- Rich semantic review UI beyond the demo panels
- IDE integration and hosted/distributed flows, which are out of v1 scope anyway
