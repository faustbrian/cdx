# Contributing

Keep changes small, explicit, and fully verified.

## Local checks

Run the full local gate before opening a pull request:

```bash
cargo fmt --check --all
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
./scripts/audit-workflow-actions.sh
./scripts/audit-supply-chain.sh
```

## Expectations

- Keep dependencies locked and intentional.
- Do not weaken lint policy to make a change pass.
- Prefer tests for behavior changes.
- Keep workflow actions pinned to immutable commits.
