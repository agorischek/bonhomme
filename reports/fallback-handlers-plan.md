# Plan: Fallback Handlers (files without a full language plugin)

**Status:** proposed. The router/blob foundation should land alongside the
multi-plugin work the Go plan introduces (it supersedes that plan's per-repo
language idea). Structured-data and tree-sitter tiers follow.
**Companion reading:** [go-plugin-plan.md](go-plugin-plan.md) (the registry this
evolves), [structural-identity-plan.md](structural-identity-plan.md),
[core-premise.md](core-premise.md).

## Reframe: the fallback is the foundation, not the edge case

Real repositories are polyglot — a TypeScript repo is also JSON, Markdown, YAML,
lockfiles, and images — and early on *most* files have no plugin. So the right
model is inverted from "what do we do when a plugin is missing":

> The fallback is the floor that always works. Language plugins are progressive
> enhancements layered on top of the files they can claim.

Two consequences shape everything below:

- **The fallback is just another `LanguagePlugin`.** A `BlobHandler` implements the
  same trait and treats a file opaquely, so the engine never has a "no plugin"
  branch — it resolves to the blob handler as the catch-all.
- **Dispatch is per-*file*, not per-*repo*.** A single repo mixes TS, JSON, and
  blobs at once, so the Go plan's per-repository `language` is too coarse. We need a
  router that maps each file to a handler, with blob as the terminal default.

**The merge engine needs zero changes.** `analyze_merge` operates on symbol
write-sets, not on language. A blob file is a symbol whose body is the whole file;
two branches editing different files touch different symbols (`SAFE_MERGE`), the
same file touches one symbol (`CONFLICT`). Mixed merges (a TS method + a README)
already work. Fallbacks slot into the existing op-log/merge model for free.

## The principle that decides what's in and what's out

Every tier below **merges and conflicts at whole-unit granularity and never
auto-resolves *inside* a unit.** That is the line. bonhomme's premise is "merge
operations, not text; surface conflicts, do not guess." A handler may make units
*finer* (file → section → symbol), but it must never line-merge within a unit,
because:

1. it reintroduces the text merge the premise explicitly rejects, and
2. blobs have **no validator** (you cannot compile a README), so a silently-wrong
   in-unit merge ships uncaught — strictly more dangerous than semantic merges,
   which have the `tsc`/`go build` gate.

This is the crisp reason **Tier C (tree-sitter) is in and Tier E (blob line-merge)
is out** — C conflicts on same-unit edits, E guesses inside a unit with no safety
net. See "Explicitly out of scope" below.

## Foundation: the per-file router

- A `HandlerRegistry`: an ordered set of handlers. Each declares what it `claims`
  (by extension first, content sniff as a tiebreak). Resolution returns the first
  claimant; the blob handler claims everything (terminal).
- Replace the Go plan's per-repo `repositories.language` with a **`handler` tag in
  each file symbol's metadata** (`"typescript" | "go" | "json" | "markdown" |
  "blob"`). Import tags each file; render / `recover_operations` re-dispatch by that
  tag, so a blob is never TS-rendered and vice versa.
- Trait impact is minimal. A `Handler` *is* a `LanguagePlugin` plus a `claims`
  predicate; the engine groups the tree by handler and calls each on its file
  subset, concatenating the resulting operations. `read_source_tree` moves up to the
  router (read everything; partition; dispatch) since no single language owns "read
  the tree" in a polyglot repo.

This router is the keystone: it is the honest proof that `LanguagePlugin` is a real
boundary, and it is shared by every tier.

## Tier D — Blob handler (the floor; implement first)

The universal fallback. Any bytes round-trip; files survive; merge at file
granularity; conflicts stay principled.

- **claims:** everything (terminal).
- **import:** each file → one symbol `{ kind: "file", name: path, body: content,
  metadata: { handler: "blob", path } }`, no children.
- **render:** emit `body` verbatim.
- **recover/diff:** the file *is* one symbol; changed content → `UpdateSymbol{body}`.
  Identity is the **path** — no comments, no AST. This is the simplest possible case
  under the structural-identity model (path is the identity).
- **validate:** no-op `Ok` — the merge gate simply does not apply to blob files.
- **binary / large files:** start with base64 in `body`; note a follow-up to store
  content-addressed (hash in the operation, bytes in a CAS / `attachments`) to keep
  the op-log JSONB from bloating, git/Datomic-style.
- **merge:** unchanged engine. Same file on two branches → `OVERLAPPING_SYMBOL_WRITE`
  → `CONFLICT`. Different files → clean.

The cost of the floor: two agents editing different lines of the same README
*conflict*. That is acceptable and correct ("don't guess"); we make it finer for
files that deserve it with Tier B — never with line-merge.

## Tier B — Structured-data handlers (the cheap, high-value win)

Most "non-code" files are still structured. Decomposing them by key-path or section
gives real concurrency (different keys/sections merge) for the files agents co-edit
most (`package.json`, configs, docs).

- **JSON / TOML:** parse to a tree; model top-level keys (and optionally nested
  subtrees) as symbols whose body is the serialized value; container nesting follows
  the object tree. Re-render by canonical re-serialization (determinism, like
  `gofmt`). Validate = well-formedness (it parses); schema validation if a schema is
  known.
- **Markdown:** parse (pulldown-cmark) into sections by heading; each heading +
  its content (until the next same-or-higher heading) is a symbol; nesting follows
  heading depth. Re-render by concatenating sections in order.
- **Identity:** the key-path / heading-path *is* the identity — structural, stable,
  comment-free.

**Fidelity caveat (the main risk).** Re-serializing JSON/YAML/TOML can lose comments
and formatting; YAML especially is comment-heavy. Options, in order of preference:
keep formatting-sensitive formats (YAML, commented TOML) as **span-preserving**
(store each key's original text span and splice edits, like the tree-sitter tier) or
leave them at the blob tier until a format-preserving parser is wired in. Start with
**JSON** (low formatting stakes) and **Markdown** (naturally text-preserving by
section span); treat YAML/TOML as span-based or deferred.

## Tier C — Tree-sitter structural-lite (breadth without per-language effort)

One dependency (tree-sitter + grammars) buys *top-level-symbol* granularity for
hundreds of languages that lack a full plugin.

- **claims:** files for a language with a loaded grammar and no full plugin.
- **parse:** CST → extract top-level named declarations via per-grammar node-type
  heuristics (`function_definition`, `class_definition`, …) as symbols; body = the
  source span; inter-symbol text becomes preamble.
- **render:** splice symbol bodies + preamble back by stored byte ranges.
- **identity:** `(kind, name)` structural.
- **validate:** none (cannot compile) — but, crucially, it still conflicts on
  same-symbol edits and never line-merges within a symbol, so it stays on the safe
  side of the principle.

Lower priority than B: it is breadth, not value-density. It is also the natural
on-ramp to a future full plugin for any given language (start tree-sitter, graduate
to a hand-tuned plugin with a validator when the language earns it).

## Explicitly out of scope: Tier E (blob + line merge)

**Not recommended — declined, not deferred-by-default.** Line-merging inside an
opaque file would:

1. reintroduce the text merge bonhomme exists to transcend;
2. be the *only* mechanism that auto-resolves *inside* a unit — i.e. guesses —
   against the "surface conflicts, don't guess" principle;
3. do so with **no validator** to catch a silently-wrong merge (unlike semantic
   merges, which have a compiler gate); and
4. largely duplicate value Tier B delivers better — structured files get key/section
   merge; genuinely unstructured line-text co-editing is rare enough that
   file-granularity conflict is an acceptable price.

If a team specifically wants git-style behavior on plain text, expose it later as an
explicit, off-by-default `--text-merge` strategy clearly labeled "not the bonhomme
model" — but it is not part of this plan and not a priority.

## Cross-cutting concerns

- **Validation is per-handler.** The merge gate validates only files whose handler
  can validate; blob/markdown/tree-sitter files pass. A merge is "semantically
  validated where it can be, opaque elsewhere" — and review should *say* so.
- **Error fallback ≠ no-plugin fallback.** A `.ts` file the TS plugin cannot parse
  should degrade to a blob with a warning, not sink the whole import — one broken
  file shouldn't fail the repo. (Configurable: degrade vs reject.)
- **Transparency.** Tag each file's handler and surface it: "5 files merged
  semantically, 3 as opaque blobs." Degradation must be visible, never silent.
- **No merge-engine changes anywhere.** Every tier produces ordinary symbol
  operations; `analyze_merge` and the replay/validate path are untouched.

## Phased delivery

- **F0 — Router + Blob.** `HandlerRegistry`, per-file `handler` metadata tag,
  `read_source_tree` at the router, `BlobHandler` (import/render/recover/validate).
  Polyglot repos work; the existing TS plugin keeps its files. Reconcile with the Go
  plan (registry becomes the router; drop per-repo `language`).
- **F1 — JSON + Markdown handlers.** Tier B for the two highest-value, lowest-risk
  formats. Key-path / section symbols; canonical / span-preserving render.
- **F2 — Error-fallback + transparency.** Degrade-on-parse-failure to blob;
  per-handler validation gating; handler reporting in state/review output.
- **F3 — Binary/large via CAS.** Content-addressed blob storage to keep the op-log
  lean.
- **F4 — Tree-sitter tier.** Breadth across grammar-supported languages; span-based
  render; top-level-symbol granularity.
- **F5 — (optional) span-preserving YAML/TOML.** Promote comment-heavy structured
  formats from blob to structured once a format-preserving approach is in place.

## Testing

- Polyglot import/render/round-trip: a repo with `.ts`, `.json`, `.md`, and a binary
  asset → each file routed to its handler → render is byte-stable.
- Blob merge: same file on two branches conflicts; different files merge.
- JSON/Markdown merge: different keys/sections merge; same key/section conflicts;
  re-serialization is deterministic.
- Error-fallback: a malformed `.ts` degrades to a blob with a warning, repo still
  imports.
- Transparency: state output reports the handler per file and the
  semantic-vs-opaque merge breakdown.
- Mixed merge: a TS method edit + a README edit on independent branches → clean.

## Risks & open questions

- **Structured-format fidelity** (comments/formatting on re-serialize) — the main
  Tier B risk; mitigated by starting with JSON/Markdown and going span-preserving
  for the rest.
- **Handler precedence / claims conflicts** — a file claimed by two handlers
  (e.g., `.ts` vs a future `.tsx`); make the registry order explicit and testable.
- **Binary in the op-log** — base64 is a stopgap; CAS (F3) is the real answer; until
  then, cap inline blob size.
- **Per-file vs per-repo dispatch** settles the Go plan's open question (polyglot
  repo) — adopt per-file as the default; a single file in two languages is not a
  thing, so per-file is sufficient.
- **Graduation path** — tree-sitter → full plugin for a language should reuse the
  same symbol kinds where possible so existing repos upgrade cleanly rather than
  re-importing.
