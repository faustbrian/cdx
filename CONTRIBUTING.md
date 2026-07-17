# Contributing

Keep changes small, explicit, and fully verified.

## Local checks

Run the full local gate before opening a pull request:

```bash
just ci
```

Or run the component checks directly:

```bash
cargo fmt --check --all
cargo test --locked --all-targets --all-features
cargo build --release --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
./scripts/test-policy-audits.sh
./scripts/audit-dependency-policy.sh
./scripts/audit-docs-policy.sh
./scripts/audit-host-dependencies.sh
./scripts/audit-license-policy.sh
./scripts/audit-package.sh
./scripts/audit-readme-badges.sh
./scripts/audit-release-archive.sh
./scripts/audit-release-workflow.sh
./scripts/audit-readme-coverage.sh
./scripts/audit-workflow-actions.sh
./scripts/audit-workflow-metadata.sh
./scripts/audit-coverage.sh
./scripts/audit-supply-chain.sh
```

## Expectations

- Keep dependencies locked and intentional.
- Keep host requirements minimal and Rust-native; do not add package-manager or service-manager setup to automation.
- Do not weaken lint policy to make a change pass.
- Prefer tests for behavior changes.
- Keep workflow actions pinned to immutable commits.
- Keep Dependabot and dependency-review policy aligned with the enforced audit.
- Keep published verification docs aligned with the enforced local gate.
- Keep release attestation verification bound to the exact source digest being
  published.
- Keep measured library coverage above the enforced line/function/region floor.
