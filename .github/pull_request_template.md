## Summary

- explain the user-visible or maintainer-visible change
- call out any data-model, CLI, or workflow changes

## Verification

- [ ] `cargo fmt --check --all`
- [ ] `cargo test --locked --all-targets --all-features`
- [ ] `cargo build --release --locked --all-features`
- [ ] `cargo clippy --locked --all-targets --all-features -- -D warnings`
- [ ] `./scripts/test-policy-audits.sh`
- [ ] `./scripts/audit-dependency-policy.sh`
- [ ] `./scripts/audit-docs-policy.sh`
- [ ] `./scripts/audit-host-dependencies.sh`
- [ ] `./scripts/audit-license-policy.sh`
- [ ] `./scripts/audit-package.sh`
- [ ] `./scripts/audit-readme-badges.sh`
- [ ] `./scripts/audit-release-archive.sh`
- [ ] `./scripts/audit-release-workflow.sh`
- [ ] `./scripts/audit-workflow-actions.sh`
- [ ] `./scripts/audit-workflow-metadata.sh`
- [ ] `./scripts/audit-supply-chain.sh`
- [ ] `./scripts/audit-coverage.sh`
- [ ] `./scripts/audit-readme-coverage.sh`
