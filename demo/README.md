# bonhomme demo

React/Vite dashboard for the local bonhomme Rust API.

This app is the rich development demo for simulations. The lightweight repo-scoped explorer ships
in the core CLI as `bonhomme explore`.

Start the API from the repository root:

```sh
export BONHOMME_TSC="$PWD/demo/node_modules/.bin/tsc"
cargo run -p bonhomme -- server
```

Then run the demo:

```sh
npm install
npm run dev
```

The dashboard can reset the demo repository, simulate queued branches, launch a real Codex swarm that lands work as agents finish, merge queued simulated work live, or run the backend stress test.

## Real Codex swarm

With the Rust API and Vite dashboard running, open the Workbench tab, set `Agents` to the count you
want, and click `Real Codex`. The API launches that many local Codex workers in the background; the
dashboard polls as branches move from editing to landed. Each worker merges as soon as its semantic
slice is recovered. If Bonhomme reports a semantic conflict, that worker checks out the real
Bonhomme conflict workspace, edits only the `resolved/` projection, submits `/api/conflicts/:id/resolve`,
and lets Bonhomme recover and validate the resolution operations.

Manual fallback from this directory:

```sh
npm run swarm:codex -- --api-base http://127.0.0.1:3030 --agents 30 --concurrency 30 --model gpt-5.3-codex-spark --merge
```

Each worker gets its own bonhomme branch and isolated `OrderService.ts` slice. The script launches
`codex exec`, asks for a mixed set of additions, body edits, renames, deletes, reference updates,
and intentional conflicts, then applies the edited slices back through the running API.

For a fast smoke test without spending model calls:

```sh
npm run swarm:codex -- --api-base http://127.0.0.1:3030 --agents 3 --dry-run --merge
```

Use `--agents 22 --dry-run --merge` when you specifically want the smoke test to exercise the
intentional duplicate-work conflict and real conflict-resolution path.
