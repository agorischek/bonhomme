# bonhomme

bonhomme is a Rust/Postgres prototype of a semantic source control system for TypeScript repositories.

The operation log is authoritative. The semantic graph and rendered TypeScript files are reconstructed from immutable operations.

See [docs/spec-coverage.md](docs/spec-coverage.md) for the current implementation coverage and remaining gaps.

## Run locally

```sh
docker compose up -d postgres
cp .env.example .env
cargo run -p bonhomme -- demo reset
cargo run -p bonhomme -- server
```

If Docker Desktop hangs while pulling public images with `error getting credentials`, bypass the broken Desktop credential helper for this project:

```sh
mkdir -p /tmp/bonhomme-docker-anon
env DOCKER_CONFIG=/tmp/bonhomme-docker-anon /Applications/Docker.app/Contents/Resources/cli-plugins/docker-compose up -d postgres
```

In another terminal:

```sh
cd demo
npm install
npm run dev
```

Open the Vite URL and use the controls to spawn many agent branches, watch them submit semantic operations, and merge them into `main`.

For faster compiler validation during merges, point bonhomme at the demo TypeScript binary:

```sh
export BONHOMME_TSC="$PWD/demo/node_modules/.bin/tsc"
```

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
bonhomme branch create --repo bonhomme-demo --name agent-a --base main
bonhomme demo spawn --count 32
bonhomme demo merge-all
bonhomme simulate --agents 128
bonhomme slice create --repo bonhomme-demo --branch main --symbol OrderService > slice.json
bonhomme slice apply --repo bonhomme-demo --slice-id <slice-id> --modified edited-slice.json
bonhomme render --repo bonhomme-demo --branch main --out rendered
bonhomme query find-symbol --repo bonhomme-demo --branch main --name OrderService
bonhomme query find-dependencies --repo imported-ts --branch main --name displayName
```

`bonhomme simulate` resets the demo repository, creates deterministic agent branches, merges them in a stable shuffled order, validates replay/render determinism, and runs `tsc` on the final rendered TypeScript.

`bonhomme slice create` persists slice provenance: the branch, operation position, and root symbols used to render the editable projection. The returned slice is clean TypeScript without `bonhomme:symbol` or `bonhomme:file` identity comments. `bonhomme slice apply --slice-id` uses the stored base graph to recover semantic operations from the edited slice. The older `--original/--modified` apply path is still available as a legacy two-file diff.
