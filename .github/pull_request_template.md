## Summary

- explain the user-visible or maintainer-visible change
- call out any data-model, CLI, or workflow changes

## Verification

- [ ] `cargo fmt --check`
- [ ] `cargo test --all-targets --all-features`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `./scripts/audit-workflow-actions.sh`
- [ ] `./scripts/audit-supply-chain.sh`
