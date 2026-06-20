# Plan: A Browser for a bonhomme Repository

**Status:** split into two surfaces. `bonhomme explore` now ships a lightweight,
repo-scoped Axum HTML explorer in the core CLI. It discovers config from the current
repo root, serves one logical bonhomme repository/branch, supports branch switching,
server-side `?as_of=`, symbol tree/detail, references, rendered files, and recent
operation history. The React/Vite app remains the rich development demo over
`/api/demo/*`, including agent simulations, branch dashboards, semantic review
experiments, and graph visualizations.
Remaining: deeper repo-agnostic JSON read APIs, paginated operation feeds, and a
first-class per-symbol-history endpoint instead of the current composed in-memory
view.
**Companion reading:** [core-premise.md](core-premise.md) (files are projections),
[structural-identity-plan.md](structural-identity-plan.md) (the clean-render toggle),
[fallback-handlers-plan.md](fallback-handlers-plan.md) (blobs in the tree).

## Goal

The GitHub file explorer's equivalent for bonhomme — a GUI for browsing what's in a
repository. But bonhomme is not file-centric: the source of truth is a **semantic
graph + an operation log**, with files as one projection. So the explorer's atom is
the **symbol, not the file**, and the tree is keyed by **stable identity** — a
rename relabels a node, it does not delete-and-re-add it.

The browser's job is to make three axes GitHub's file tree lacks first-class:

- **Containment** — the symbol tree (file → class → method). The direct file-explorer
  analog.
- **References** — "who calls this / what does this depend on." Sourcegraph-style
  semantic navigation.
- **Time** — the op-log: scrub to any revision and the whole UI re-materializes.
  Datomic-console / time-travel browsing, which the engine already supports.

Plus the reason bonhomme exists: **branches/agents** — many concurrent agent branches
with merge status.

The defining principle: **everything is a lens on the same selection.** Selecting a
symbol in the tree, the code, the graph, or the log highlights it everywhere.

## What's in the DB (and therefore browsable)

- **Structure:** repositories → branches (`base_branch_id`, `base_position`, status)
  → tasks → changesets (`created_by`, `task_id`) → operations (immutable, `position`,
  `op_type`, `payload`) → attachments (`PromptAttachment`: model + prompt).
- **Semantic graph** (materialized): `SymbolNode { id, parent_id, kind, name, body,
  metadata, ordinal }`, `ReferenceNode { id, from, to, kind, ordinal }`.
- **Projections:** rendered files, the `graph_cache`.

The graph gives the tree and detail; the log gives history and time-travel; the
references give navigation; branches + changesets + attachments give the agent and
provenance story.

## The views

Each is a panel/screen; all coordinate on the current selection + the current
"as-of" revision + the current branch.

1. **Symbol tree** (the file-explorer equivalent). Containment tree of symbols, kind
   icons, identity-keyed nodes. Badges for branch-local status (`new`, `conflict`,
   `blob`). Blobs (Markdown, images, un-pluginned files) appear as childless leaves.
2. **Symbol detail.** Rendered code for the selected symbol/container, a metadata
   strip (kind, signature, id), and a **Code | Semantic** toggle — *Code* is the
   familiar rendered file; *Semantic* is "show this as meaning." The defining
   bonhomme affordance; GitHub only has the former.
3. **Inspector.** Three sections:
   - *References* — callers / callees / dependencies / dependents, each clickable
     (the Sourcegraph jump). Powered by the existing query methods.
   - *Provenance* — the task → changeset → prompt that created the symbol. Richer
     than `git blame`: the *intent*, not just the author.
   - *History (semantic blame)* — the operations that touched **this symbol id**,
     across branches.
4. **Operation-log timeline.** The authoritative history as a scrubber. Ticks colored
   by branch; dragging the handle sets an "as-of" revision and the entire UI
   re-materializes at that point (time-travel). Filter by branch / changeset / op
   type / symbol.
5. **Agent/branch dashboard.** Every branch (`main` + `agent-NNN`), its status
   (ready / merged / conflict), what symbols it created, and merge state. Generalizes
   the current demo's branch panels — the "many agents at once" view.
6. **Changeset review.** The semantic-diff surface from the premise's "review
   changesets, not line diffs": *"agent-017 created method X; added reference X→Y"*
   instead of a text hunk. The PR-review equivalent.
7. **Reference graph.** A focus-and-expand node-link map of symbols and `calls`
   edges — the codebase's semantic map. Lazy expansion only (never render-all).

## Defining interactions

- **Selection coordination** — one selected symbol id highlighted across tree, code,
  inspector, graph, and log.
- **Code ↔ Semantic toggle** — switch any file/symbol between its rendered text and
  its semantic representation.
- **Time scrubber** — "view as of operation N"; the whole app reflects that snapshot.
- **Branch switch / compare** — change the active branch; optionally diff two branches
  semantically (which symbols/ops differ), reusing `analyze_merge`'s notion of
  divergence.

## Read API it needs

Today's endpoints are demo-centric (`/api/demo/*`, `/api/repos/{repo}/branches/
{branch}/render`). The CLI already implements the queries
(`find-symbol/references/callers/callees/dependencies/dependents`). A general browser
needs a repo-agnostic **read** surface — almost all derivable from existing
`materialize_branch` + queries:

| Endpoint | Backed by | Status |
|---|---|---|
| `GET /repos`, `/repos/{r}/branches` | `list_branches` etc. | exists internally |
| `GET /repos/{r}/branches/{b}/graph?asOf=<n>` | `collect_branch_operations(b, Some(n))` + `materialize` | engine ready; endpoint new |
| `GET /repos/{r}/branches/{b}/render?asOf=<n>` | same, then `plugin.render` | partially exists |
| `GET /symbols/{id}` (detail + refs + provenance) | graph + `find_*` + changeset/attachment lookup | compose existing |
| `GET /symbols/{id}/history` | log filtered to ops whose `write_symbols()` ∋ id | **genuinely new query** |
| `GET /repos/{r}/branches/{b}/operations?cursor=…` | `list_own_operations` paginated | new (pagination) |
| `GET /changesets/{id}` | `list_changesets` + its operations | compose existing |

The only real new backend work is **per-symbol history** and **paginated, as-of
reads**; everything else composes what's there.

## Scale

- Lazy tree expansion (load children on expand).
- Server-side symbol search (`find-symbol` exists).
- Cursor-paginated op-log; the timeline buckets ops, fetching detail on zoom.
- Reference graph: focus node + expand neighbors; cap rendered nodes.
- Cache materializations (the `graph_cache` already does this per branch; extend for
  as-of snapshots if time-travel proves hot).

## Architecture

- **Core explorer:** `crates/bonhomme/src/explorer.rs`, served by `bonhomme explore`.
  It is Rust-only Axum HTML, repo-scoped, and configured from the discovered checkout
  root. There is no Node/Vite/React dependency in the core CLI path.
- **Demo app:** `demo/` remains a full React/Vite development lab. It can keep richer
  simulation controls and exploratory visualizations without becoming part of the
  shipped explorer.
- **Backend:** read paths are composed from `Storage` (`materialize_branch`,
  `materialize_branch_at_position`, `list_branches`, `list_operations`). Future
  dedicated JSON endpoints should mirror those same read models rather than depending
  on `/api/demo/state`.
- **Coordination:** the server-rendered explorer carries `{ branch, as_of,
  selectedSymbolId }` in query parameters; the React demo may continue using
  client-side state for richer development interactions.

## Phased delivery

- **B0 — read API foundation.** Repo/branch list, `graph?asOf`, paginated operations.
  Repo-agnostic (not `/api/demo/*`).
- **B1 — symbol tree + detail.** The explorer: tree, kind icons, identity-keyed nodes,
  rendered code in the detail pane. *Already beats a file tree.*
- **B2 — inspector.** References (clickable nav) + provenance. The semantic payoff —
  "what is this, who uses it, why does it exist."
- **B3 — op-log timeline + time-travel.** The scrubber and as-of re-materialization;
  add `GET /symbols/{id}/history`.
- **B4 — changeset review.** Semantic diff of a changeset's operations.
- **B5 — agent/branch dashboard.** Fold in / generalize the demo's branch panels;
  branch compare.
- **B6 — reference graph.** Focus-and-expand node-link map.

B1–B2 are the MVP: they answer the question a path-based tree cannot — *what is this
symbol, who uses it, and why does it exist.*

## Testing

- Tree fidelity: the symbol tree matches the materialized graph's containment for a
  branch; blobs render as childless leaves.
- Identity stability: a rename relabels the node and keeps its history/references
  (no node churn).
- Time-travel: `graph?asOf=n` equals materializing the first n operations; scrubbing
  is consistent across panels.
- References: clicking a caller navigates to it; matches CLI `find-callers`.
- Selection coordination: selecting in any panel updates all others to the same id.

## Risks & open questions

- **Time-travel cost.** Re-materializing per scrub position; mitigate with as-of
  snapshot caching and op bucketing on the timeline.
- **Graph view at scale.** A large reference graph needs focus+expand and node caps;
  full-graph layout is a non-goal.
- **Cross-branch identity.** A symbol id can appear on several branches with different
  bodies; the detail/history view must be explicit about which branch + as-of it is
  showing.
- **Blob/handler display.** Tree icons and the Code/Semantic toggle should reflect the
  per-file handler (TS vs JSON vs blob) once fallback handlers land.
- **Write actions.** This plan is read-only browsing. Editing (issue a slice, apply,
  trigger a merge) is a separate surface that should reuse the slice/merge APIs — keep
  the browser a viewer first.
