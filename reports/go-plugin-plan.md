# Plan: A Go Language Plugin

**Status:** proposed. Should land *after* the `LanguagePlugin` trait settles into
its structural-identity shape (see
[structural-identity-plan.md](structural-identity-plan.md)) so Go is built greenfield
against the final interface, with no comment-identity legacy.
**Companion reading:** [related-work.md](related-work.md) (why Go over Python/Rust/Java),
[core-premise.md](core-premise.md).

## Why Go (the short version)

Go is the truest match for bonhomme's deliberately small semantic model: flat,
named declarations; no significant whitespace; no macros; a canonical formatter
(`gofmt`); and a fast, strict compiler that makes an excellent external validator —
which is exactly the safety net the merge engine relies on. It is also heavily
agent-generated, so it delivers real value, not just a proof of concept.

Most importantly, Go is the right *second* language because it is **not
class-based**. Picking a second TS-shaped language (JS, Java) would prove little.
Go forces three things to be real rather than aspirational:

1. **A language-neutral kind taxonomy** — `struct`/`interface`/`field`/receiver
   `method` instead of TS's `class`/`method`/`property`.
2. **Containment that differs from textual layout** — a Go method
   `func (s *Server) Start()` is textually top-level but semantically a child of
   `Server`. The graph parent ≠ where the text sits. This is the sharpest test of
   "the graph is the truth; text is a projection."
3. **Multi-plugin support in the engine** — today `Storage` holds exactly one
   `Arc<dyn LanguagePlugin>`. A second language is what forces per-repository plugin
   selection to exist.

## What Go tests that TS can't

| Concern | TS today | Go forces |
|---|---|---|
| Kind vocabulary | `class`/`method`/`property` | `struct`/`interface`/`field`/`const`/receiver method — proves core treats `kind` as opaque |
| Symbol vs. text location | method nested in class body | method is top-level with a receiver → graph parent decoupled from text position |
| Parser strategy | in-process (oxc) | language-toolchain subprocess (Go's own `go/parser` + `go/types`) |
| Formatting/determinism | bespoke renderer | canonical `gofmt` — determinism for free, output is idiomatic |
| Validator strength | `tsc` (good) | `go build` (fast + strict) |
| Engine plugin selection | one hard-wired plugin | a registry keyed by repository language |

## Architecture

A new `crates/bonhomme-go` crate implementing `LanguagePlugin`, depending only on
`bonhomme-core` (mirroring `bonhomme-ts`). The engine stays language-free; the
composition root registers it.

**Parser/validator via the Go toolchain (subprocess), not a Rust AST.** Unlike
oxc (an in-process TS parser), there is no `syn`-grade Go AST in Rust — and that is
fine, even preferable here. Ship a tiny embedded Go helper that uses Go's own
`go/parser` + `go/types` to emit a JSON symbol model, and `go/format` to render
canonically. The Rust plugin shells out to it, exactly as the TS plugin shells out
to `tsc` for validation. This gives a *typed* model (real reference resolution),
reuses the subprocess pattern bonhomme already has, and is arguably *more* aligned
with "the plugin delegates to the language's own toolchain" than embedding a parser.

```text
crates/bonhomme-go/
  src/lib.rs            # GoPlugin: impl LanguagePlugin (thin Rust wrapper)
  src/model.rs          # JSON <-> graph mapping
  src/toolchain.rs       # invoke `go run <helper>` / `go build`; BONHOMME_GO override
  go-helper/             # small Go module
    main.go              # `parse` (go/parser+go/types -> JSON) and `format` (gofmt)
    go.mod
```

Requires the `go` toolchain on PATH (documented, like node/`tsc`), with a
`BONHOMME_GO` env override mirroring `BONHOMME_TSC`.

## Semantic model mapping

Go construct → bonhomme graph (the body stays opaque text, as in TS):

| Go | kind | parent | notes |
|---|---|---|---|
| source file | `file` | none | path-keyed; package clause + imports go to `preamble` metadata |
| `func Name(...)` | `function` | file | direct analog to a TS top-level function |
| `func (r T) Name(...)` | `method` | the `T` type symbol | **textually top-level, semantically a child of T** |
| `type T struct {...}` | `struct` | file | analog of `class` |
| `type T interface {...}` | `interface` | file | members are method *signatures* |
| struct field | `field` | the struct | rendered inside the struct body |
| interface method | `method` (sig only, no body) | the interface | |
| `const`/`var` (package-level) | `const`/`var` | file | |
| `import (...)` | — | — | preamble metadata, like TS imports |
| call expression in a body | `calls` reference | — | resolved precisely by `go/types` (better than the TS regex era) |

Out of scope for v1 (lives in bodies as opaque text, or deferred): generics/type
params, embedded structs/interfaces, `init` funcs, build tags, cgo, channels/
goroutines semantics, and `implements` edges (Go interfaces are satisfied
structurally — inferring those is a later enhancement, not v1).

### The kind taxonomy decision

**Each plugin owns its own `kind` vocabulary; core stays opaque.** After the recent
review, core no longer interprets kinds (the one leak, `find_callers`/`find_callees`
hard-coding `"calls"`, was parameterized). So the Go plugin can use `struct`/
`interface`/`field`/`const` freely with no core change — and that is precisely the
test: a checkpoint of this work is to **audit core + engine for any residual
TS-kind assumption** (duplicate-sibling uses `(parent, kind, name)`, which is
vocabulary-agnostic; confirm nothing else sneaks one in). If something breaks, the
abstraction was incomplete and we found it with language #2, not #5.

## Multi-plugin selection (the real engine change)

Today `Storage { plugin: Arc<dyn LanguagePlugin> }` assumes one language. Generalize:

- Add a `LanguageRegistry` (a `BTreeMap<String, Arc<dyn LanguagePlugin>>`) and a
  `repositories.language` column (default `"typescript"`).
- `Storage` holds the registry; every render/import/diff/validate resolves the
  plugin by the repository's `language`. Helper: `Storage::plugin_for(repo)`.
- The composition root (api/cli) registers `{"typescript": TypeScriptPlugin,
  "go": GoPlugin}`.
- `init`/`import` take a `--language` flag that sets the repo's language.

This is the single most consequential architectural addition; it is the honest
proof that the `LanguagePlugin` seam is a real boundary and not a one-off.

## Render

- Walk the graph deterministically (by ordinal, as TS does): file → package clause
  + imports (preamble) → consts/vars → type decls (structs with fields, interfaces
  with method sigs) → top-level funcs → **methods emitted as top-level
  `func (recv) Name()` declarations after their owning type**, even though they are
  graph-children of that type. The graph→text placement divergence is the whole
  point.
- Pipe the assembled source through `gofmt` (via the helper's `format` mode /
  `go/format`). Canonical formatting gives render determinism essentially for free
  and yields idiomatic output the agent is happy to edit.

## Validate

Write the rendered package into a temp module (`go.mod` + files), run
`go build ./...` (optionally `go vet`), capture failures. Fast and strict — a
strong external validator for the merge gate. Same shape as
`validate_typescript_files`, with a `BONHOMME_GO` override and a timeout.

## Identity recovery

Go rides directly on the structural-identity work — it implements
`recover_operations(base, scope, edited)`, matching the edited Go AST against the
graph subtree by `(kind, container, name)` with body-similarity for renames.
Methods match by `(receiver type, name)`. Because Go is greenfield here, it should
**never** emit identity comments — it is the clean-slate validation that structural
identity works without any text-carried ids at all.

## v1 subset

In: packages/files, top-level funcs, structs + fields, interfaces + method
signatures, receiver methods, package-level const/var, call references, gofmt
rendering, `go build` validation. Out (deferred): generics, embedding,
`implements` edges, init funcs, build tags, cgo.

## Phased delivery

- **G0 — toolchain spike.** The Go helper: `parse` (source → JSON symbol model via
  `go/parser`+`go/types`) and `format` (gofmt). `toolchain.rs` invokes it; detect
  `go` / honor `BONHOMME_GO`. No bonhomme integration yet.
- **G1 — import + render round-trip.** Map JSON ↔ graph; render via gofmt; validate
  via `go build`. Prove import→render→`go build` on a fixture package. Crate exists,
  not yet wired to the engine.
- **G2 — multi-plugin engine.** `LanguageRegistry` + `repositories.language` +
  `Storage::plugin_for`; `init/import --language go`. Audit core/engine for TS-kind
  assumptions. TS behavior unchanged.
- **G3 — identity recovery.** Implement `recover_operations` for Go (structural,
  comment-free), reusing the matcher patterns from the structural-identity work.
- **G4 — references + queries.** `calls` edges from `go/types`; confirm
  find-callers/callees/dependencies work cross-language.
- **G5 — simulation + docs.** A Go variant of the multi-agent simulation (agents
  add methods to a struct), update `docs/spec-coverage.md`, document the toolchain
  requirement.

## Testing

- Round-trip: import a Go package → render → `go build` clean; re-render is
  byte-identical (gofmt determinism).
- Method placement: a receiver method is a graph-child of its type but renders
  top-level; deleting the type-as-scope vs the method behaves correctly.
- Identity: rename a method (clean text) → `UpdateSymbol`, references preserved.
- Multi-plugin: a `go` repo and a `typescript` repo coexist; each resolves its
  plugin; no cross-talk.
- Cross-file references within a package resolve.
- Determinism property test: same edit → same operations.

## Risks & open questions

- **Toolchain dependency.** Parse *and* validate now need `go` installed
  (TS only needs it for validate). Acceptable and documented, but heavier for
  contributors; consider a pre-built helper binary to avoid `go run` per call.
- **Subprocess cost.** Parsing via subprocess is slower than in-process oxc; batch
  files per package and cache where possible.
- **Receiver/owner resolution.** A method's owning type may live in another file of
  the package; the helper must resolve receivers package-wide, not per-file.
- **Package vs. file model.** bonhomme is file-centric; Go is package-centric.
  v1 keeps file symbols and lets references cross files (graph refs are global);
  revisit if package-level semantics (visibility, package docs) need first-class
  modeling.
- **Generics.** Deferred, but common in modern Go; decide when type params graduate
  from opaque-signature-text to modeled.
- **Plugin selection granularity.** Per-repository language is simplest; a polyglot
  repo (Go + TS) would need per-file resolution — out of scope for v1.
