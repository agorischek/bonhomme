# Plan: bonhomme as a Local Coauth Session over Git

**Status:** proposed. The adoption wedge — a local, ephemeral semantic merge layer
for agent swarms that commits to Git, with Git as the durable source of truth.
**Companion reading:** [core-premise.md](core-premise.md) (operations are truth),
[fallback-handlers-plan.md](fallback-handlers-plan.md) (whole-tree round-trip),
[db-browser-plan.md](db-browser-plan.md) (the review UI),
[structural-identity-plan.md](structural-identity-plan.md) (round-trip fidelity).

## The reframe

"bonhomme replaces Git" is the north star, but it is an enormous, decade-long
trust-building ask. The pragmatic v1 inverts it:

> bonhomme runs **locally and ephemerally** as a *coauth session* — a coordination
> layer for many agents editing one repository between commits. **Git stays the
> committed source of truth.** A session starts by importing the working tree, lets
> agents merge their work semantically, and ends by rendering changed files back and
> letting you `git commit` as normal.

This recasts bonhomme from "a new VCS" into "a semantic merge queue for agent
swarms." The value (clean concurrent merges, validated commits, semantic review) is
immediate; the bar (be better than nothing at coordinating a swarm within a session)
is low; and Git is the backstop for everything bonhomme is not yet good at.

## Why this is the right v1

- **It hides every current weakness behind Git.** Durability, full-language
  fidelity, prototype maturity — none of them matter when the session is local and
  ephemeral and `git diff` review is the safety net. If the importer mishandles some
  construct, you catch it before committing.
- **It plays to the one real strength:** intra-session concurrency. Ten agents
  adding ten methods merge cleanly at the semantic level instead of fighting over the
  same file region.
- **It improves on raw agent edits.** A session only lands *validated, compiling*
  output (the `tsc`/`go build` gate), so Git receives cleaner commits than agents
  editing files directly, which can leave broken intermediate states.
- **Clean division of labor:** bonhomme = the agent-swarm merge queue; Git =
  inter-session, human, and durable truth. Cross-session and human-vs-agent merges
  fall back to Git's text merge — which is fine; that is not the hard part bonhomme
  exists to solve.

## The session lifecycle

```text
bonhomme session start      import the Git working tree -> session graph
                            (semantic where a plugin claims the file; blob otherwise)
        |
        v  fan out N agents, each editing slices -> semantic operations on session branches
        |
bonhomme session merge      semantic merge of agent changesets
                            (analyze_merge + replay + tsc/go build gate -> SAFE_MERGE | CONFLICT)
        |
bonhomme session review     the browser: review changesets, not line diffs
        |
bonhomme session land       render only the touched files back into the working tree
        |                    -> you `git add` / `git commit` as normal
        v
bonhomme session discard    throw away the local session database
```

Plus **`bonhomme session rebase`**: if upstream Git moves mid-session (you pulled),
re-base the in-flight agent changesets onto the new working tree *semantically* —
cleaner than a text rebase of half-finished agent work. This reuses the engine's
existing `base_position` / merge machinery.

## How it fits what is already built

- **Local embedded database (libSQL / Turso):** the session is a disposable
  `.bonhomme/session.db` next to the repo — no cloud, no durability requirement,
  thrown away after the commit. This is exactly the local-embedded-SQLite use case.
- **The blob / fallback handler is load-bearing here.** A session must round-trip the
  *whole* working tree: semantic files become a graph, everything else (configs,
  binaries, un-pluginned languages) becomes a blob that renders **byte-for-byte
  verbatim** — so untouched files produce zero diff.
- **The validation gate** means a session can only land output that compiles.
- **The browser is the session review UI** — "review changesets, not line diffs" is
  literally how you approve a coauth session before it commits.
- **The merge engine needs no changes** — sessions are just branches + changesets in
  the existing op-log model.

## The make-or-break requirement: diff-clean round-trip

This is the crux, and it is bonhomme's current thinnest area. For "Git is the truth,"
a session must commit a diff containing **only the agents' intended changes** — no
spurious reformatting noise. Two parts:

1. **Untouched files → byte-identical.** The blob handler gives this for free (path is
   the identity, body is verbatim), *and* `land` renders only the files an agent
   actually touched. Solved by construction.
2. **Touched files → must match the repo's canonical formatter.** bonhomme's renderer
   emits *canonical* formatting, not the original's, so a touched file comes back
   reformatted unless the render matches the repo's style. **The unlock is aligning
   bonhomme's render with the repo's formatter:** Go is essentially there (`gofmt`);
   TypeScript needs prettier-compatible output. In a repo already normalized by
   prettier/gofmt — i.e. most modern repos — bonhomme's render *matches* and
   touched-file diffs are clean. In a bespoke-formatting repo you get reformatting
   noise.

**This is the top engineering priority.** Get it right and the model holds together;
get it wrong and every session spams reformatting diffs and no one trusts it.

## What it delivers — and what it does not (yet)

- **Delivers now:** semantic merge of concurrent agent work; validated/compiling
  commits; semantic (changeset-level) review; semantic rebase onto a moving base.
- **Does not deliver yet:** *persistent* semantic identity and history. When a session
  commits to Git and is discarded, the operation log and stable symbol IDs are gone.
  The next session re-imports from scratch (IDs are re-derived deterministically from
  path + name, so unchanged names stay stable, but a committed rename loses the
  thread — same as Git). The Unison-like "this method, across every rename, forever"
  dream requires bonhomme to own the durable log — that is the replace-Git endgame,
  reached via the graduation path below, not the session wedge.

## The graduation path (session → truth, incrementally)

No flag day. Thicken the semantic layer inside Git over time:

- **Phase 1 — files only.** `land` commits rendered files. Git sees normal code.
  bonhomme is purely an ephemeral session tool.
- **Phase 2 — identity sidecar.** `land` also writes the operation log + symbol-id map
  as Git sidecar data (a `.bonhomme/` tree, `git notes`, or commit trailers, keyed by
  commit). Now identity **survives across sessions** while Git still carries the bytes
  and remains fully usable by anyone who ignores bonhomme.
- **Phase 3 — bonhomme as interface.** The sidecar grows rich enough (full op log,
  semantic history) that Git is effectively storage/transport and bonhomme is the
  interface. You have "replaced Git" without anyone abandoning it.

Trust accrues session by session; the semantic layer thickens commit by commit; there
is never a moment where someone bets their history on a prototype.

## The `.bonhomme/` layout

```text
.bonhomme/
  session.db        # local libSQL, ephemeral — the live session (gitignored)
  log/              # (Phase 2+) committed op log + symbol-id map, keyed by commit
                    #   so the next session can re-attach identity to renamed/moved code
```

`session.db` is gitignored and disposable. The `log/` sidecar (Phase 2+) is the only
thing that crosses the commit boundary, and it is additive — a repo that ignores it
behaves exactly like a normal Git repo.

## Phased delivery

- **S0 — round-trip fidelity gate (the prerequisite).** `session start` + `session
  land` on a clean repo with *no* agent edits must produce an empty `git diff`:
  byte-identical untouched (blob) files, and formatter-clean touched files. This is
  the make-or-break; build it first and gate everything on it.
- **S1 — single-session swarm.** Agent fan-out → semantic merge within the session →
  `tsc`/`go build` gate → `land` only touched files. The core value.
- **S2 — review + rebase.** Wire the browser as `session review`; implement `session
  rebase` onto a moved working tree.
- **S3 — identity sidecar (graduation Phase 2).** Persist the op log + symbol-id map
  to `.bonhomme/log` on `land`; re-attach on the next `start` so identity survives
  renames across sessions.
- **S4 — thicken toward semantic history (Phase 3 north star).**

## Testing

- **Empty round-trip:** `start` then `land` with no edits → `git diff` is empty
  (untouched blobs byte-identical; touched-none).
- **Single touched method:** one agent edits one method → diff shows only that change,
  no reformatting elsewhere in the file.
- **Multi-agent merge:** N agents add independent methods → one clean `SAFE_MERGE` →
  one clean commit.
- **Semantic rebase:** upstream adds a file mid-session → the session lands cleanly on
  top of the new base.
- **Blob fidelity:** binaries and config files survive `start`/`land` byte-identical.
- **Conflict surfacing:** two agents add the same method name → `CONFLICT` shown in
  review, nothing lands until resolved.

## Risks & open questions

- **Formatter alignment (the crux).** Prettier-compatible TypeScript rendering is real
  work; until it lands, the session model is clean only in gofmt/prettier-normalized
  repos. Scope target accordingly.
- **Whole-tree vs scoped sessions.** Import the entire working tree, or just the paths
  a task touches? Whole-tree is simplest for round-trip integrity but heavier.
- **Conflict resolution UX.** When a session has a semantic `CONFLICT`, how does a
  human resolve it — pick a side in the browser, re-prompt an agent, or drop to text?
- **Mixed human + agent edits.** If a human edits files directly (outside bonhomme)
  during a session, the session's base is stale — detect and `rebase`, or re-import.
- **Sidecar ergonomics (Phase 2+).** `git notes` vs a tracked `.bonhomme/log` tree;
  keeping it from bloating history; making it invisible to non-bonhomme users.
- **Session DB lifecycle.** `.bonhomme/session.db` must be gitignored and cleaned up;
  decide crash-recovery semantics (resume vs discard a half-finished session).
