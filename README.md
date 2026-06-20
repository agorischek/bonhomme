# bonhomme

bonhomme is a Rust/Postgres prototype of a semantic source control system for TypeScript/JavaScript, Go, Rust, Python, and C# repositories.

The operation log is authoritative. The semantic graph and rendered source files are reconstructed from immutable operations.

See [docs/spec-coverage.md](docs/spec-coverage.md) for the current implementation coverage and remaining gaps.

## Run locally

```sh
cp .env.example .env
cargo run -p bonhomme -- import --repo bonhomme --path examples/typescript-basic --reset
cargo run -p bonhomme -- explore --repo bonhomme --open
```

`bonhomme explore` is the core lightweight explorer. It runs one local Axum instance for the
repository in the current checkout, discovers `bonhomme.toml` from that repo root, and writes the
last started URL to `.bonhomme/explorer.json`. If no storage URL is configured, bonhomme uses the
project-local embedded Turso database at `.bonhomme/bonhomme.db`.

The React demo is still available as a development dashboard for simulations:

```sh
cargo run -p bonhomme -- demo reset
cargo run -p bonhomme -- server
```

In another terminal:

```sh
cd demo
npm install
npm run dev
```

Open the Vite URL and use the controls to spawn many agent branches, watch them submit semantic operations, and merge them into `main`.

If you prefer Postgres for either flow, start the local database and set `DATABASE_URL` or
`[storage].database_url` in `bonhomme.toml`:

```sh
docker compose up -d postgres
```

If Docker Desktop hangs while pulling public images with `error getting credentials`, bypass the broken Desktop credential helper for this project:

```sh
mkdir -p /tmp/bonhomme-docker-anon
env DOCKER_CONFIG=/tmp/bonhomme-docker-anon /Applications/Docker.app/Contents/Resources/cli-plugins/docker-compose up -d postgres
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

`BONHOMME_TSC` takes precedence over `[toolchain].typescript`; `[toolchain].tsc` is accepted as an alias.

Go support uses the local Go toolchain for parsing, `gofmt`, and validation. Set `BONHOMME_GO` if `go` is not on `PATH`.

Rust support uses `syn` in-process for parsing, `prettyplease` for deterministic formatting, and `cargo check` for validation. Set `BONHOMME_CARGO` if `cargo` is not on `PATH`.

Python support uses tree-sitter in-process for parsing and `python3 -m py_compile` for validation. Set `BONHOMME_PYTHON` or `[toolchain].python` if `python3` is not on `PATH`.

C# support uses tree-sitter in-process for parsing and `dotnet build` for validation. Set `BONHOMME_DOTNET` or `[toolchain].dotnet` if `dotnet` is not on `PATH`.

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

- `Reset` rebuilds the demo repository from the initial `OrderService`. After reset, `Run` is disabled because there are no agent branches yet.
- `Spawn` creates simulated agent branches. Once branches are ready, `Run` becomes enabled.
- `Run` merges ready branches into `main` one by one so you can watch the graph, operation log, merge review, and rendered TypeScript update.
- `Stress` runs the backend simulation in one shot and reports safe merges, conflicts, replay determinism, render determinism, and `tsc` validation.
- `Conflict twins` makes some agents intentionally create the same semantic method name so the merge engine reports a conflict instead of guessing.

### Panels

- `Agent Editors` shows the simulated agent branches and the semantic slice each branch wants to apply.
- `Semantic Graph` shows the materialized graph reconstructed from operation replay.
- `Rendered TypeScript` shows the compatibility projection that TypeScript tooling can compile.
- `Operation Log` shows append-only operations stored in Postgres.
- `Merge Review` shows operation-level merge results rather than line diffs.

For the clearest run:

```text
Reset -> Spawn -> Run
```

Then watch `OrderService` grow realistic methods while the graph and operation log advance.

## CLI examples

```sh
bonhomme init --name bonhomme-demo
bonhomme import --repo imported-ts --path examples/typescript-basic --reset
bonhomme import --repo imported-go --path examples/go-basic --reset
bonhomme import --repo imported-rust --path examples/rust-basic --reset
bonhomme import --repo imported-python --path examples/python-basic --reset
bonhomme import --repo imported-csharp --path examples/csharp-basic --reset
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
bonhomme slice create --repo bonhomme-demo --branch main --symbol OrderService > slice.json
bonhomme slice apply --repo bonhomme-demo --slice-id <slice-id> --modified edited-slice.json
bonhomme render --repo bonhomme-demo --branch main --out rendered
bonhomme query find-symbol --repo bonhomme-demo --branch main --name OrderService
bonhomme query find-dependencies --repo imported-ts --branch main --name displayName
bonhomme query find-callees --repo imported-go --branch main --name Summary
bonhomme query find-callees --repo imported-rust --branch main --name summary
```

`bonhomme simulate` resets the TypeScript demo repository, creates deterministic agent branches, merges them in a stable shuffled order, validates replay/render determinism, and runs `tsc` on the final rendered TypeScript. `bonhomme simulate --language go` runs the same merge-engine stress path against a Go `OrderService` and validates with `go build`; `bonhomme simulate --language rust` does the same for a Rust `OrderService` and validates with `cargo check`.

`bonhomme session start` imports a working tree into `.bonhomme/session.db` and records the active repo, branch, and base operation position in `.bonhomme/session.json`. `bonhomme session land` writes only files touched after that base position. In-place land is gated by `[git].write_back = true` in `bonhomme.toml`; `--out` is always allowed.

`bonhomme slice create` persists slice provenance: the branch, operation position, and root symbols used to render the editable projection. The returned slice is clean TypeScript without `bonhomme:symbol` or `bonhomme:file` identity comments. `bonhomme slice apply --slice-id` uses the stored base graph to recover semantic operations from the edited slice. The older `--original/--modified` apply path is still available as a legacy two-file diff.

Stored-slice applies attach a `SliceRecoveryAttachment` to the created changeset. It records the slice ID, base position, whether the branch had advanced, appended operation IDs, and a compact operation-decision summary for review.

If multiple symbols in the same slice are renamed and rewritten so identity cannot be recovered deterministically, bonhomme rejects the apply instead of asking agents to preserve hidden anchors in TypeScript.
