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

## CLI examples

```sh
bonhomme init --name bonhomme-demo
bonhomme import --repo imported-ts --path examples/typescript-basic --reset
bonhomme branch create --repo bonhomme-demo --name agent-a --base main
bonhomme demo spawn --count 32
bonhomme demo merge-all
bonhomme simulate --agents 128
bonhomme render --repo bonhomme-demo --branch main --out rendered
bonhomme query find-symbol --repo bonhomme-demo --branch main --name OrderService
bonhomme query find-dependencies --repo imported-ts --branch main --name displayName
```

`bonhomme simulate` resets the demo repository, creates deterministic agent branches, merges them in a stable shuffled order, validates replay/render determinism, and runs `tsc` on the final rendered TypeScript.
