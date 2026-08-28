# Spec Coverage

bonhomme currently implements a runnable v1 prototype, not a complete production implementation of the draft.

## Implemented

- Rust CLI/API named `bonhomme`
- PostgreSQL operation-log storage
- repositories, branches, tasks, changesets, operations, attachments
- immutable operations: `CreateSymbol`, `DeleteSymbol`, `UpdateSymbol`, `CreateReference`, `DeleteReference`
- deterministic graph replay from operation records
- validation for duplicate IDs, dangling parents, dangling references, and duplicate sibling symbols
- TypeScript/JavaScript import for conservative `.ts`, `.tsx`, `.js`, and `.jsx` subsets: files, classes, methods, properties, top-level functions, and call references
- per-file handler registry with TypeScript, Go, Rust, Python, C#, Java, PHP, Ruby, Kotlin, Swift, Elixir, structured text, tree-sitter, and blob fallback handlers
- Go import for conservative `.go` subsets: files, structs, fields, interfaces, top-level functions, receiver methods, package-level const/var/type declarations, and `calls` references resolved through `go/types`
- Go rendering through the helper's `go/format` path and validation through `go build ./...`
- Rust import for conservative `.rs` subsets: files, structs, enums, traits, fields, variants, top-level functions, impl methods, trait methods, const/static/type declarations, raw fallback items, and conservative `calls` references
- Rust rendering through `prettyplease` and validation through `cargo check`
- Python import for conservative `.py`/`.pyi` subsets: files, classes, methods, class/module attributes, top-level functions, and conservative `calls` references
- Python rendering with indentation-aware source generation and validation through `python -m py_compile`
- C# import for conservative `.cs` subsets: files, namespaces, classes/interfaces/structs/enums, fields, properties, constructors, methods, and conservative `calls` references
- C# rendering with namespace-aware source generation and validation through `dotnet build`
- Java import for conservative `.java` subsets: files, packages/imports, classes, records, interfaces, annotations, enums, nested types, fields/constants, annotation elements, enum constants, constructors, methods, initializer blocks, overload-distinct method symbols, and conservative `calls` references
- Java rendering with package/import-preserving source generation and validation through `javac`
- PHP import for conservative `.php` subsets: files, namespaces/imports, top-level functions, classes/interfaces/traits/enums, properties, constants, trait uses, enum cases, methods, and conservative `calls` references
- PHP rendering with namespace/import-preserving source generation and validation through `php -l`
- Ruby import for conservative `.rb` subsets: files, modules/classes, nested modules/classes, top-level methods, instance/singleton methods, constants/attributes, and conservative receiver-aware `calls` references
- Ruby rendering with `end`-block source generation and validation through `ruby -c`
- Kotlin import for conservative `.kt`/`.kts` subsets: files, packages/imports, classes/interfaces/objects, nested objects/classes, top-level functions, functions, properties, constructors/initializers, type aliases, and conservative `calls` references
- Kotlin rendering with package/import-preserving source generation and validation through `kotlinc`
- Swift import for conservative `.swift` subsets: files/imports, classes/structs/enums/protocols/actors/extensions, nested types, top-level functions, functions, properties, initializers, enum cases, type aliases, and conservative `calls` references
- Swift rendering with import-preserving source generation and validation through `swiftc -parse`
- Elixir import for conservative `.ex`/`.exs` subsets: files, modules, grouped functions/macros by `name/arity`, module directives, and conservative local/remote `calls` references
- Elixir rendering with module/function source generation and validation through `elixirc`
- clean TypeScript slice rendering backed by stored branch/base-position/root-symbol provenance
- clean Go slice rendering without identity comments
- clean Rust slice rendering without identity comments
- graph-anchored slice apply for method additions/updates/deletes, top-level function additions/updates/deletes, and new TypeScript files in rendered slices
- graph-anchored Go slice apply for conservative function/method body updates/additions/deletes, receiver-method additions, struct fields, interface method signatures, package-level values, and call-reference updates
- graph-anchored Rust slice apply for conservative function/method body updates/additions/deletes, type/field/variant/trait-method/package-value additions and deletions, and call-reference updates
- stale stored-slice apply with operation-level conflict detection before append
- `SliceRecoveryAttachment` provenance for stored-slice applies
- deterministic rejection of ambiguous structural identity recovery instead of in-text anchors
- legacy two-file slice diff for comment-bearing projections
- operation-level merge with deterministic `SAFE_MERGE` or `CONFLICT`
- first-class semantic conflict records for merge and stale slice-apply conflicts, with CLI/API
  list/show/checkout/resolve/abandon commands, base/current/proposed/resolved workspaces, recovery
  faithfulness checks, and `ConflictResolutionAttachment` provenance
- real Codex/demo swarm path that checks out semantic conflicts, edits `resolved/`, resolves through
  Bonhomme, and marks resolved merge branches as landed in the dashboard
- language toolchain validation after merge and during `validate` (`tsc` for TypeScript/JavaScript, `go build` for Go, `cargo check` for Rust, `py_compile` for Python, `dotnet build` for C#, `javac` for Java, `php -l` for PHP, `ruby -c` for Ruby, `kotlinc` for Kotlin, `swiftc -parse` for Swift, `elixirc` for Elixir)
- persistent graph/render cache keyed by branch operation count and operation-id fingerprint
- queries: find symbol, references, callers, callees, dependencies, dependents
- local coauth session start/review/land scaffold backed by `.bonhomme/session.db`
  and `.bonhomme/session.json`, with changed-file write-back from the recorded
  session base
- React/Vite visual demo for many agent branches and semantic merge review
- deterministic simulation command/API for many TypeScript, Go, or Rust agent branches, final replay/render checks, and compiler validation
- unit tests, a property-style merge commutativity test, a 512-agent in-memory simulation test, and importer round-trip coverage

## Still Incomplete

- Full TypeScript/JavaScript AST and type-checker-backed semantic model
- General import fidelity for every TypeScript/JavaScript construct
- General import fidelity for every Go construct
- General import fidelity for every Rust construct
- General import fidelity for every Python construct
- General import fidelity for every C# construct
- General import fidelity for every Java construct
- General import fidelity for every PHP construct
- General import fidelity for every Ruby construct
- General import fidelity for every Kotlin construct
- General import fidelity for every Swift construct
- General import fidelity for every Elixir construct
- Go generics, embedding, build tags, cgo, `init`, package-level documentation, and `implements` edges
- Rust macros, modules, visibility edge cases, generics/where-clause reconstruction beyond the conservative renderer, attributes/docs, trait-resolution-backed call edges, and Cargo-workspace-aware validation
- Python nested classes/functions, decorators beyond preservation in signatures, dataclass/descriptor semantics, imports/package awareness, dynamic dispatch, and type-checker-backed references
- C# overloads, partial types, records, delegates/events, attributes/docs, generics edge cases, project-aware validation, and Roslyn-backed references
- Java modules, anonymous/local classes, overload-resolved call edges, generics/annotation semantics beyond preservation, Maven/Gradle-aware validation, and compiler-backed references
- PHP anonymous classes, namespace alias semantics beyond preservation, dynamic method/function calls, attributes beyond preservation, Composer-aware validation, and static-analysis-backed references
- Ruby mixins/metaprogramming, blocks/lambdas as symbols, dynamic dispatch, refinements, autoload-aware namespaces, Bundler-aware validation, and static-analysis-backed references
- Kotlin sealed/value/data-class semantics beyond preservation, delegated properties, extension-function receiver resolution, overload-resolved calls, Gradle-aware validation, and compiler-backed references
- Swift extensions/protocol conformance semantics beyond preservation, property observers, result builders/macros, overload-resolved calls, SwiftPM-aware validation, and compiler-backed references
- Elixir protocols/behaviours, imports/aliases beyond suffix matching, guards, captures, sigils/docs, umbrella-project-aware validation, and compiler-backed references
- Semantic diff for class edits, file deletes, reference updates, properties, interfaces, enums, decorators, and namespaces
- Cross-package Go repositories and package-aware rendering beyond the current conservative package/file model
- Cross-crate Rust repositories and crate/module-aware rendering beyond the current conservative file model
- Arbitrary branch DAG merge support beyond the current direct branch-to-target workflow
- Broader semantic conflict-resolution coverage across every language construct and conflict class
- Session-native apply/merge/rebase/discard/resume commands and a committed
  `.bonhomme/log` identity sidecar
- Broader randomized simulation suite across destructive edits, deletes, updates, and non-method symbols
- Rich semantic review UI beyond the demo panels
- IDE integration and hosted/distributed flows, which are out of v1 scope anyway
