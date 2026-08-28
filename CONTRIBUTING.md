# Contributing

Bonhomme is a Rust workspace with a React/TypeScript demo. Keep changes focused, explain semantic
and storage compatibility implications, and add regression evidence for behavior changes.

## Local setup

Install stable Rust with `rustfmt` and Clippy, Node.js 24, npm, and any language toolchains needed by
the code you are changing. Postgres is required for Postgres storage conformance tests.

Before opening a pull request, run the relevant checks from CI:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo run --locked -p bonhomme -- git benchmark
considered check
(cd demo && npm ci && npm run lint && npm run build)
```

Storage or migration changes must also pass the Postgres conformance test documented in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml). Never commit credentials, production data,
machine-local paths, generated databases, or audit logs.

## Pull requests

- Describe the problem, approach, risks, and verification performed.
- Keep migrations additive and compatible with both supported backends.
- Update user and operations documentation when commands or deployment behavior change.
- Record intentional repository-policy exceptions in `.considered.yaml` with a narrow reason and a
  limit no higher than the measured value.
- Report security issues privately according to [SECURITY.md](SECURITY.md).
