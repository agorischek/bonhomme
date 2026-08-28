# bonhomme

bonhomme is a Rust/Postgres prototype of a semantic source control system for TypeScript/JavaScript, Go, Rust, Python, C#, Java, PHP, Ruby, Kotlin, Swift, and Elixir repositories.

The operation log is authoritative. The semantic graph and rendered source files are reconstructed from immutable operations.

See [docs/spec-coverage.md](docs/spec-coverage.md) for the current implementation coverage and remaining gaps.
For organization handoff and destination security settings, use the
[transfer checklist](docs/transfer-checklist.md).

## Prerequisites

- A current stable Rust toolchain with `rustfmt` and Clippy
- Node.js 24 and npm for the React demo and TypeScript validation tests
- Optional language toolchains for the languages you want Bonhomme to validate

## Run locally

```sh
cargo run -p bonhomme -- import --path examples/typescript-basic --reset
cargo run -p bonhomme -- explore --open
```

`bonhomme explore` is the core lightweight explorer. It runs one local Axum instance for the
repository in the current checkout, discovers `bonhomme.toml` from that repo root, and writes the
last started URL to `.bonhomme/explorer.json`. If no storage URL is configured, bonhomme uses the
project-local embedded Turso database at `.bonhomme/bonhomme.db`.

Repo-scoped commands infer the logical repository name from the active session manifest, then the
current repo root directory. They also use the active session database automatically when one exists.
Pass `--repo` or `--database-url` only when addressing a different logical repository or storage
target.

## Git compatibility and shadow evaluation

Git remains the byte-history ledger. Import any named commit without checking it out, export a
Bonhomme branch to a standalone Git worktree with a reproducible tree ID, or compare both merge
engines without moving Git refs:

```sh
bonhomme git import --git-dir . --revision HEAD~1 --repo evaluation --reset
bonhomme git export --repo evaluation --branch main --out exported-evaluation
bonhomme git shadow --git-dir . --base <base-commit> --ours <target-commit> --theirs <source-commit>
bonhomme git benchmark
```

Shadow and benchmark commands are storage-free and emit machine-readable JSON. Git import/export
currently accept regular non-executable (`100644`) files; symlinks, gitlinks, executable-mode
preservation, and non-UTF-8 paths fail explicitly rather than degrading silently. The versioned
[semantic merge corpus](benchmarks/semantic-merge/README.md) declares expected Git outcomes,
semantic outcomes, canonical trees, blob routing, and unsafe-result counts.

For repo-specific agent instructions:

```sh
cargo run -p bonhomme -- agents
```

The React demo is still available as a development dashboard for simulations:

```sh
cargo run -p bonhomme -- demo reset
cargo run -p bonhomme -- server
```

Loopback development remains zero-config. Binding the API to a non-loopback address fails closed
unless `BONHOMME_API_TOKEN` is set. Authenticated deployments must also set
`BONHOMME_API_SUBJECT`, `BONHOMME_API_ROLE=reader|writer|admin`, and exactly one
`BONHOMME_API_TENANTS` value. Each server process is a single-tenant trust boundary; wildcard and
multi-tenant values are rejected. Clients send the token as `Authorization: Bearer …`; an optional
`X-Bonhomme-Tenant` header must match the configured tenant. Authorization headers are marked
sensitive before request tracing. Production tenants must use separately access-controlled
databases as described in [`docs/operations.md`](docs/operations.md). Server resource policy is
non-secret configuration:

```toml
[server]
require_auth = true
max_request_bytes = 2097152
request_timeout_seconds = 120
max_concurrent_requests = 64
```

Authenticated callers can inspect their effective identity at `/api/whoami` and process-local
request/error/latency counters at `/api/metrics`. Mutating requests and direct mutating CLI commands
are recorded as hash-chained, fsynced intent/outcome pairs under `.bonhomme/audit.jsonl`; a damaged
chain prevents reuse. The local chain is durable and tamper-evident, not independently immutable:
production configuration requires an external WORM export/anchoring policy.
Durable, tenant-scoped worker jobs are available under `/api/repos/{repo}/jobs`; see
[`docs/durable-jobs.md`](docs/durable-jobs.md) for the lease and retry protocol.
Deployment preflight, backup/restore, credential rotation, audit anchoring, and incident handling are
covered by the executable commands and procedures in [`docs/operations.md`](docs/operations.md).

In another terminal:

```sh
cd demo
npm install
npm run dev
```

Open the Vite URL and use the controls to create agent branches, watch them submit semantic operations, and merge them into `main`.

For the coworker demo with real Codex workers, start the API and Vite dashboard as above, open the
Workbench tab, set `Agents` to the count you want, and click `Real Codex`. The API launches that
many local `codex exec` workers in the background; the dashboard polls as branches move from editing
to landed. Each worker attempts to merge as soon as its semantic slice is recovered; if a semantic
conflict appears, the worker checks out Bonhomme's real conflict workspace, edits the `resolved/`
projection, and submits the resolution through Bonhomme's conflict resolver.

Manual fallback:

```sh
cd demo
npm run swarm:codex -- --api-base http://127.0.0.1:3030 --agents 30 --concurrency 30 --model gpt-5.3-codex-spark --merge
```

The real swarm creates the selected number of bonhomme branches, renders an isolated
`OrderService.ts` slice for each one, and launches `codex exec` in each worker directory. The default
mission mix includes additions, body edits, method renames, method deletes, reference updates, and a
few intentional conflicts. Use `--api-base http://127.0.0.1:3030 --dry-run --agents 3 --merge` for a
quick local smoke test that exercises the slice/apply/merge path without launching Codex, or
`--api-base http://127.0.0.1:3030 --dry-run --agents 22 --merge` to exercise the real
conflict-resolution path.

If you prefer Postgres for either flow, start the local database and set `DATABASE_URL` or
`[storage].database_url` in `bonhomme.toml`:

```sh
docker compose up -d postgres
```

If Docker Desktop hangs while pulling public images with `error getting credentials`, bypass the broken Desktop credential helper for this project:

```sh
docker_config_dir="$(mktemp -d)"
DOCKER_CONFIG="$docker_config_dir" docker compose up -d postgres
```

TypeScript validation runs an existing compiler only. By default, Bonhomme invokes `tsc` from
`PATH`; it never installs TypeScript with `npx` or another package manager. To use the demo
compiler, set:

```sh
export BONHOMME_TSC="$PWD/demo/node_modules/.bin/tsc"
```

You can also configure a TypeScript-compatible compiler per repo in `bonhomme.toml`:

```toml
[toolchain]
typescript = "tsgo" # or a path to tsc
```

Relative toolchain paths are resolved from the repo root containing `bonhomme.toml`, so a repo can
point at an already-installed local compiler such as `docs/node_modules/.bin/tsc`.

`BONHOMME_TSC` takes precedence over `[toolchain].typescript`; `[toolchain].tsc` is accepted as an alias.

Toolchain validation is an explicit confidence gate, not required for session startup. By default,
`bonhomme session start` checks semantic graph invariants and skips language build/typecheck gates.
Run `bonhomme session start --validate toolchain` when you want startup to fail unless the rendered
projection passes language toolchains. Repos can opt into that policy:

```toml
[validation]
session_start = "toolchain" # default: "none"
```

Go support uses the local Go toolchain for parsing, `gofmt`, and validation. Set `BONHOMME_GO` if `go` is not on `PATH`.

Rust support uses `syn` in-process for parsing, `prettyplease` for deterministic formatting, and `cargo check` for validation. Set `BONHOMME_CARGO` if `cargo` is not on `PATH`.

Python support uses tree-sitter in-process for parsing and `python3 -m py_compile` for validation. Set `BONHOMME_PYTHON` or `[toolchain].python` if `python3` is not on `PATH`.

C# support uses tree-sitter in-process for parsing and `dotnet build` for validation. Set `BONHOMME_DOTNET` or `[toolchain].dotnet` if `dotnet` is not on `PATH`.

Java support uses tree-sitter in-process for parsing and `javac` for validation. Set `BONHOMME_JAVAC`, `[toolchain].java`, or `[toolchain].javac` if `javac` is not on `PATH`.

PHP support uses tree-sitter in-process for parsing and `php -l` for validation. Set `BONHOMME_PHP` or `[toolchain].php` if `php` is not on `PATH`.

Ruby support uses tree-sitter in-process for parsing and `ruby -c` for validation. Set `BONHOMME_RUBY` or `[toolchain].ruby` if `ruby` is not on `PATH`.

Kotlin support uses tree-sitter in-process for parsing and `kotlinc` for validation. Set `BONHOMME_KOTLINC`, `[toolchain].kotlin`, or `[toolchain].kotlinc` if `kotlinc` is not on `PATH`.

Swift support uses tree-sitter in-process for parsing and `swiftc -parse` for validation. Set `BONHOMME_SWIFTC`, `[toolchain].swift`, or `[toolchain].swiftc` if `swiftc` is not on `PATH`.

Elixir support uses tree-sitter in-process for parsing and `elixirc` for validation. Set `BONHOMME_ELIXIRC`, `[toolchain].elixirc`, or `[toolchain].elixir` if `elixirc` is not on `PATH`.

## Demo walkthrough

The Vite demo is a visual simulation of many coding agents editing the same TypeScript class at once.

The starting repository contains a rendered `OrderService` class. That file is not the source of truth; it is generated from bonhomme's append-only operation log. Each simulated agent branch proposes a small domain-shaped change, such as adding `carrierRoutingPlan`, `paymentRiskSignal`, or `refundReadiness` to `OrderService`.

Each agent change is represented as semantic operations:

```text
CreateSymbol(method, "carrierRoutingPlan")
CreateReference(calls, carrierRoutingPlan -> displayName)
CreateReference(calls, carrierRoutingPlan -> listOrders)
```

The demo is meant to show that many agents can modify the same class without bonhomme merging text. Independent method additions merge as operations. Duplicate semantic additions become conflicts.

### Controls

- `Agents` sets how many branches the simulated or real swarm should create.
- `Conflicts` makes some agents intentionally create the same semantic method name so the merge engine reports a conflict instead of guessing.
- `Simulate` creates deterministic branches awaiting merge without launching Codex.
- `Real Codex` resets the demo, launches local `codex exec` workers with the selected agent count, and lets each worker merge or repair its work as soon as it finishes.
- `Stress test` runs the backend simulation in one shot and reports safe merges, conflicts, replay determinism, render determinism, and `tsc` validation.
- `Merge queue` manually merges queued simulated branches into `main` one by one so you can watch the graph, operation log, merge review, and rendered TypeScript update.
- `Reset` rebuilds the demo repository from the initial `OrderService`.

### Panels

- `Agent Editors` shows the simulated agent branches and the semantic slice each branch wants to apply.
- `Semantic Graph` shows the materialized graph reconstructed from operation replay.
- `Rendered TypeScript` shows the compatibility projection that TypeScript tooling can compile.
- `Operation Log` shows append-only operations stored in Postgres.
- `Merge Review` shows operation-level merge results rather than line diffs.

For the clearest run:

```text
Reset -> Simulate -> Merge queue
```

Then watch `OrderService` grow realistic methods while the graph and operation log advance.

## CLI examples

```sh
bonhomme init --name bonhomme-demo
bonhomme agents
bonhomme import --repo imported-ts --path examples/typescript-basic --reset
bonhomme import --repo imported-go --path examples/go-basic --reset
bonhomme import --repo imported-rust --path examples/rust-basic --reset
bonhomme import --repo imported-python --path examples/python-basic --reset
bonhomme import --repo imported-csharp --path examples/csharp-basic --reset
bonhomme import --repo imported-java --path examples/java-basic --reset
bonhomme import --repo imported-php --path examples/php-basic --reset
bonhomme import --repo imported-ruby --path examples/ruby-basic --reset
bonhomme import --repo imported-kotlin --path examples/kotlin-basic --reset
bonhomme import --repo imported-swift --path examples/swift-basic --reset
bonhomme import --repo imported-elixir --path examples/elixir-basic --reset
bonhomme explore --repo imported-ts --branch main --open
bonhomme session start --repo bonhomme-session --path examples/typescript-basic --reset --no-validate
bonhomme session review --repo bonhomme-session --open
bonhomme session land --repo bonhomme-session --out rendered-session
bonhomme branch create --repo bonhomme-demo --name agent-a --base main
bonhomme demo spawn --count 32
bonhomme demo merge-all
bonhomme simulate --agents 128
bonhomme simulate --language go --agents 128
bonhomme simulate --language rust --agents 128
bonhomme slice create --repo bonhomme-demo --branch main --symbol OrderService
bonhomme slice apply --repo bonhomme-demo --title "Edit OrderService" --agent agent-a
bonhomme slice create --repo bonhomme-demo --branch main --symbol OrderService --json # legacy protocol JSON
bonhomme render --repo bonhomme-demo --branch main --out rendered
bonhomme query find-symbol --repo bonhomme-demo --branch main --name OrderService
bonhomme query find-dependencies --repo imported-ts --branch main --name displayName
bonhomme query find-callees --repo imported-go --branch main --name Summary
bonhomme query find-callees --repo imported-rust --branch main --name summary
```

`bonhomme simulate` resets the TypeScript demo repository, creates deterministic agent branches, merges them in a stable shuffled order, validates replay/render determinism, and runs `tsc` on the final rendered TypeScript. `bonhomme simulate --language go` runs the same merge-engine stress path against a Go `OrderService` and validates with `go build`; `bonhomme simulate --language rust` does the same for a Rust `OrderService` and validates with `cargo check`.

`bonhomme session start` imports a working tree into `.bonhomme/session.db` and records the active repo, branch, and base operation position in `.bonhomme/session.json`. `bonhomme session land` writes only files touched after that base position. In-place land is gated by `[git].write_back = true` in `bonhomme.toml`; `--out` is always allowed.

`bonhomme slice create` persists slice provenance: the branch, operation position, and root symbols used to render the editable projection. By default it checks out real source files under `.bonhomme/slices/<branch>/<slice-id>/`, records that workspace as the active slice, and keeps Bonhomme metadata in `.bonhomme-slice.json` instead of hiding it in source comments. `bonhomme slice apply` reads the active checked-out slice, uses the stored base graph to recover semantic operations, and appends those operations to the branch. Pass `--out <dir>` / `--dir <dir>` to choose an explicit workspace, or `--json` / `--modified` for the legacy JSON protocol and two-file diff path.

Stored-slice applies attach a `SliceRecoveryAttachment` to the created changeset. It records the slice ID, base position, whether the branch had advanced, appended operation IDs, and a compact operation-decision summary for review.

If multiple symbols in the same slice are renamed and rewritten so identity cannot be recovered deterministically, bonhomme rejects the apply instead of asking agents to preserve hidden anchors in TypeScript.
