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
- per-file handler registry with TypeScript, Go, and blob fallback handlers
- Go import for conservative `.go` subsets: files, structs, fields, interfaces, top-level functions, receiver methods, package-level const/var/type declarations, and `calls` references resolved through `go/types`
- Go rendering through the helper's `go/format` path and validation through `go build ./...`
- clean TypeScript slice rendering backed by stored branch/base-position/root-symbol provenance
- clean Go slice rendering without identity comments
- graph-anchored slice apply for method additions/updates/deletes, top-level function additions/updates/deletes, and new TypeScript files in rendered slices
- graph-anchored Go slice apply for conservative function/method body updates/additions/deletes, receiver-method additions, struct fields, interface method signatures, package-level values, and call-reference updates
- stale stored-slice apply with operation-level conflict detection before append
- `SliceRecoveryAttachment` provenance for stored-slice applies
- deterministic rejection of ambiguous structural identity recovery instead of in-text anchors
- legacy two-file slice diff for comment-bearing projections
- operation-level merge with deterministic `SAFE_MERGE` or `CONFLICT`
- language toolchain validation after merge and during `validate` (`tsc` for TypeScript, `go build` for Go)
- persistent graph/render cache keyed by branch operation count and operation-id fingerprint
- queries: find symbol, references, callers, callees, dependencies, dependents
- React/Vite visual demo for many agent branches and semantic merge review
- deterministic simulation command/API for many TypeScript or Go agent branches, final replay/render checks, and compiler validation
- unit tests, a property-style merge commutativity test, a 512-agent in-memory simulation test, and importer round-trip coverage

## Still Incomplete

- Full TypeScript AST and type-checker-backed semantic model
- General import fidelity for every TypeScript construct
- General import fidelity for every Go construct
- Go generics, embedding, build tags, cgo, `init`, package-level documentation, and `implements` edges
- Semantic diff for class edits, file deletes, reference updates, properties, interfaces, enums, decorators, and namespaces
- Cross-package Go repositories and package-aware rendering beyond the current conservative package/file model
- Arbitrary branch DAG merge support beyond the current direct branch-to-target workflow
- Broader randomized simulation suite across destructive edits, deletes, updates, and non-method symbols
- Rich semantic review UI beyond the demo panels
- IDE integration and hosted/distributed flows, which are out of v1 scope anyway
