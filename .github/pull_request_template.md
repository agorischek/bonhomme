## Summary

<!-- What problem does this solve, and what changed? -->

## Risk and compatibility

<!-- Note storage migrations, semantic/rendering changes, API or Git compatibility, and rollback. -->

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] Relevant Rust tests pass
- [ ] `considered check`
- [ ] Demo lint/build run when affected
- [ ] Documentation and migration/benchmark evidence updated when affected
- [ ] No secrets, production data, or machine-local paths added
