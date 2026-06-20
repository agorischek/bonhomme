# Related Work

## Summary

bonhomme is not a single new idea. It is a synthesis of several well-developed
lineages, aimed at a target most of them were not designed for: many AI agents
editing one codebase concurrently.

This document places bonhomme next to its closest conceptual neighbors —
**Unison**, **Pijul** (and Darcs), and **Mergiraf** (and the wider family of
structured-merge tools) — and then the broader traditions it draws on. The goal
is honest positioning: what bonhomme borrows, where it genuinely differs, and
what these more mature systems do better today.

It is worth restating up front (see [core-premise.md](core-premise.md)): bonhomme
is a runnable prototype, not a finished system. Most of the systems below are
real, used, and far more complete. The comparison is about *ideas and shape*, not
maturity.

## The axes that matter

Every system below can be located along a few axes. bonhomme's particular
position on all of them at once is what makes it distinct.

1. **Source of truth** — what is canonical: file snapshots, text patches, a
   content-addressed store, or an append-only log of operations.
2. **Unit of change** — line hunks, AST subtrees, content hashes, or semantic
   operations (`CreateSymbol`, `CreateReference`, …).
3. **Identity** — how a thing keeps its identity across edits: line position,
   content hash, tree-matching heuristics, or a stable symbol ID separate from
   its name.
4. **Merge model** — line merge, patch commutation, tree merge, CRDT
   auto-resolution, or operation replay with explicit conflicts.
5. **Language coupling** — language-agnostic, tied to one language, or pluggable
   per language.
6. **Editing model** — direct text editing, a structure/projectional editor, or
   editing a projection that is diffed back into operations.
7. **Intended editor** — humans at human scale, or many agents at once.

## Unison — code is content-addressed structure, not files

[Unison](https://www.unison-lang.org/) is bonhomme's closest cousin on the
deepest axis: **code is not text in files**. A Unison definition is identified by
the hash of its (normalized) syntax tree, stored in a codebase database, with
*names held separately as metadata*.

Shared with bonhomme:

- **Identity is structural, not textual.** In Unison a definition's hash is its
  identity; in bonhomme a symbol's UUID is. In both, a rename is a metadata
  change, not a destructive edit — so renames and reformatting produce *no merge
  conflict and no diff noise*. This is exactly bonhomme's "separate identity from
  presentation" claim.
- **Files are an export format**, not the source of truth. Unison renders text on
  demand from the codebase; bonhomme renders TypeScript on demand from the graph.
- **The store is a database**, not a tree of files.

Where they differ:

- **Unison is its own language.** Its model works because the language was
  co-designed with the content-addressed store. bonhomme instead tries to wrap an
  *existing* language (TypeScript) behind a `LanguagePlugin`, which is far messier
  — bonhomme now uses Oxc to parse TypeScript syntax, but still has to translate
  someone else's language into its own intentionally smaller semantic model.
- **Identity source.** Unison identity *is* the content hash, so two structurally
  identical definitions are literally the same object. bonhomme assigns explicit
  symbol UUIDs, so two methods with identical bodies remain distinct identities.
  bonhomme's model is closer to "objects with IDs"; Unison's is "values keyed by
  content."
- **Concurrency motivation.** Unison targets human authoring and distributed
  deployment; bonhomme targets many concurrent agents.

If you want one sentence: **Unison proves the "code is structured data with stable
identity" half of bonhomme's premise, in a setting where the language is yours to
design.**

## Pijul and Darcs — changes are first-class and merge by a theory

[Pijul](https://pijul.org/) (and its predecessor
[Darcs](http://darcs.net/)) make *changes/patches* the primary objects, with a
sound theory of how they compose. Independent changes **commute**; conflicts are
first-class values rather than textual `<<<<<<<` markers.

Shared with bonhomme:

- **Change-centric, not snapshot-centric.** Both treat the history of changes as
  primary and the working state as derived. bonhomme's "operation log is
  authoritative; the graph is a materialized view" is the same instinct.
- **Commutation is the basis of safe merging.** Pijul merges cleanly precisely
  when changes are independent. bonhomme's claim that two agents adding different
  methods to the same class merge safely is the semantic-level version of the same
  property — and bonhomme's property test literally asserts that independent
  method additions commute.
- **Principled conflicts over guessing.** Both refuse to silently invent a
  resolution. bonhomme reports `CONFLICT`; Pijul represents the conflict
  explicitly and lets you resolve it as another change.

Where they differ:

- **Granularity of a "change."** Pijul's patches are still fundamentally about
  *lines/bytes* (with a rigorous model around them). bonhomme's operations are
  *semantic* — "create a method named X under class Y" — so two edits that touch
  adjacent lines of the same file do not even appear to overlap.
- **Soundness vs. pragmatism.** Pijul's value is a *provably* associative,
  order-independent merge derived from its patch algebra. bonhomme has no such
  proof; it leans on deterministic replay plus a compiler gate (`tsc`) as an
  external validator. bonhomme's `analyze_merge` is a heuristic classifier backed
  by a replay-and-validate safety net, not a theory.

One sentence: **Pijul shows what a sound, commutation-based merge theory looks
like; bonhomme wants the same "independent changes just merge" feel but at the
semantic level, and pays for it with a compiler check instead of a proof.**

## Mergiraf, SemanticMerge, and structured merge — merge at the syntax level

This family keeps git's text-snapshot model but makes *merging* structure-aware.

- [**Mergiraf**](https://mergiraf.org/) is a recent (2024) syntactic merge driver
  that uses tree-sitter grammars to resolve conflicts git's line merge cannot, and
  is pluggable across languages via those grammars.
- **SemanticMerge** (from the makers of Plastic SCM / Unity Version Control)
  parsed files to ASTs and merged at the method/declaration level, handling moved
  and renamed methods.
- **Spork**, **JDime / FSTMerge**, and the academic *structured / semistructured
  merge* line do tree-matching-based merge, largely for Java.

Shared with bonhomme:

- **Merge happens on structure, not lines**, so independent edits to different
  members of the same file/class merge without conflict.
- **Language-pluggable merge.** Mergiraf-via-tree-sitter is the closest analog to
  bonhomme's `LanguagePlugin`: the merge engine is generic; per-language knowledge
  is injected.

Where they differ:

- **Where structure lives.** These tools *recover* structure by parsing text at
  merge time; the canonical artifact is still the text file. bonhomme makes the
  structure (the operation log and graph) canonical and *renders* text. So
  Mergiraf reconstructs identity heuristically (tree matching) on every merge,
  while bonhomme carries identity explicitly as symbol IDs that never have to be
  re-inferred.
- **Scope.** These are merge (and sometimes diff) tools. bonhomme is a whole
  store: log, graph, queries, rendering, slices, and merge. The merge *layer* is
  the part that most resembles Mergiraf; the rest has no analog here.

One sentence: **Mergiraf is the closest cousin to bonhomme's merge layer
specifically — generic engine, per-language plugin — but it re-derives structure
from text, where bonhomme treats structure as the source of truth.**

## The broader lineage

bonhomme also inherits from several traditions that are not source control at all.

- **Event sourcing / CQRS, and Datomic.** bonhomme is essentially event sourcing
  applied to a codebase: an append-only log of immutable facts, current state as a
  replayable materialized view, with a cache that can always be discarded and
  rebuilt. [Datomic](https://www.datomic.com/)'s "database as a value" and
  immutable datoms are the same architecture at the storage layer.
- **Code-fact graphs: Kythe and Glean.** bonhomme's semantic graph (symbol nodes,
  reference edges, `find-callers/callees/dependencies`) is a small version of a
  code-knowledge graph like [Kythe](https://kythe.io/) or Meta's
  [Glean](https://glean.software/) — nodes and `defines`/`ref`/`calls` edges you
  can query. CodeQL belongs here too ("code as a queryable database").
- **Projectional / structure editors: JetBrains MPS.** bonhomme's *slice* — an
  editable projection that is diffed back into operations — is projectional
  editing applied to version control. [MPS](https://www.jetbrains.com/mps/) is the
  canonical "the model is the source of truth; the text is a projection" system.
- **CRDTs: Automerge, Yjs, and tree-move CRDTs.** Operation-based concurrent
  editing is the substrate cousin. The key divergence: CRDTs aim to be
  *conflict-free* (auto-resolve), while bonhomme deliberately surfaces a semantic
  `CONFLICT` and refuses to guess. Kleppmann's *highly-available move operation for
  replicated trees* is directly relevant to bonhomme's containment tree.
- **Mergeable structured stores: Irmin and Dolt.**
  [Irmin](https://irmin.org/) is a git-like store where *merge is defined per data
  type* rather than by text — very close to "define how operations merge instead
  of merging lines." [Dolt](https://www.dolthub.com/) is "git for a SQL database":
  branch/diff/merge at the row level. Both share bonhomme's "merge structured data,
  not text" instinct, on different substrates.
- **Image-based environments: Smalltalk, Lisp.** The oldest root of "files are a
  projection": code historically lived in an object image edited through browsers,
  not files.

## At a glance

| System | Source of truth | Unit of change | Identity | Merge model | Language coupling |
|---|---|---|---|---|---|
| **bonhomme** | operation log | semantic operation | explicit symbol ID | op replay + explicit `CONFLICT` + `tsc` gate | pluggable (`LanguagePlugin`) |
| Git | file snapshots | line hunk | line position | three-way text merge | agnostic (text) |
| Unison | content-addressed store | definition (by hash) | content hash | structural; renames are metadata | own language |
| Pijul / Darcs | patch set | line/byte patch | patch + position | commutation theory, conflicts first-class | agnostic (text) |
| Mergiraf | file snapshots | AST subtree | tree-matching heuristic | structured (tree-sitter) merge | pluggable (grammars) |
| SemanticMerge | file snapshots | declaration / method | parsed AST + matching | method-level structured merge | per-language parsers |
| Datomic | fact log | datom | entity ID | n/a (DB, not SCM) | n/a |
| Kythe / Glean | derived index | fact | node ticket | n/a (read-only graph) | per-language indexers |
| CRDT (Automerge/Yjs) | op log (replicated) | CRDT op | replica-assigned ID | conflict-free auto-merge | agnostic |
| Irmin | git-like store | typed value | path/hash | per-type merge function | agnostic (typed) |

## What bonhomme borrows from each

- From **Unison**: structural identity decoupled from names; files as a rendered
  view.
- From **Pijul/Darcs**: changes as first-class, independent-changes-commute,
  conflicts you do not paper over.
- From **Mergiraf / structured merge**: structure-level merging behind a
  per-language plugin.
- From **Datomic / event sourcing**: an authoritative immutable log with derived,
  disposable views.
- From **Kythe / Glean**: a queryable symbol-and-reference graph.
- From **MPS**: editing a projection rather than the canonical artifact.

## What is genuinely distinctive

No neighbor occupies bonhomme's exact point, and two things in particular have no
clean precedent in the same package:

1. **Stacking all of the above at once** — an event-sourced log (Datomic-shaped)
   of *semantic* operations (Mergiraf-shaped merge) over a graph with explicit
   identity (Unison-shaped), edited via projections (MPS-shaped), behind a language
   plugin. Each idea exists elsewhere; the combination, aimed at source control,
   does not.
2. **The intended editor is a fleet of AI agents, not humans.** This reframes the
   merge problem. Most agentic coding setups today give each agent a git worktree
   and let a human resolve text conflicts. bonhomme's bet is that if agents submit
   *semantic operations* instead of file patches, independent work merges cleanly
   and genuine collisions surface as precise, reviewable conflicts — and that the
   system should refuse to use AI to *invent* a merge resolution, keeping the SCM
   itself deterministic and auditable.

## What these systems do better (honest limitations)

- **Soundness.** Pijul has a real theory of merge. bonhomme has a heuristic
  classifier plus a replay-and-`tsc` safety net — pragmatic, not proven.
- **Language fidelity.** Unison and the AST-based mergers work on real parse
  trees. bonhomme now imports and diffs TypeScript syntax through Oxc ASTs, but
  its semantic model still covers a conservative TypeScript subset; full-language
  and type-checker-backed fidelity is explicitly unfinished.
- **Maturity and scale.** Git, Datomic, Kythe/Glean, and Plastic SCM are
  production systems handling enormous repositories. bonhomme is a prototype whose
  largest exercise is an in-memory multi-agent simulation.
- **Generality.** Mergiraf and Irmin are already multi-language / multi-type.
  bonhomme has the *seam* for it (`LanguagePlugin`, now a real crate boundary) but
  ships only a TypeScript plugin.

## Takeaway

The three nearest cousins, by axis:

- **Unison** — for "code is structured data with stable identity, not text files."
- **Pijul / Darcs** — for "changes are first-class and merge by a principle, not by
  line diffs."
- **Mergiraf** — for "merge at the syntactic/semantic level, per language."

bonhomme's wager is that combining their best ideas — and pointing the result at
many concurrent agents rather than human-scale editing — is worth the loss of
each system's individual rigor or maturity. Whether that wager pays off is exactly
what the prototype does not yet prove.
