run: fmt
    cargo run --release

run-dev: fmt
    cargo run

install:
    cargo install --path . --locked

print-zsh-integration:
    cat contrib/cdx.zsh

build: fmt
    cargo build --release --locked

build-dev: fmt
    cargo build

lint:
    ./scripts/test-policy-audits.sh
    ./scripts/audit-lint-policy.sh
    ./scripts/audit-dependency-policy.sh
    ./scripts/audit-docs-policy.sh
    ./scripts/audit-host-dependencies.sh
    ./scripts/audit-license-policy.sh
    ./scripts/audit-package.sh
    ./scripts/audit-readme-badges.sh
    ./scripts/audit-workflow-actions.sh
    ./scripts/audit-release-workflow.sh
    ./scripts/audit-workflow-metadata.sh
    cargo fmt --check --all

policy-tests:
    ./scripts/test-policy-audits.sh

clippy:
    cargo clippy --locked --all-targets --all-features -- -D warnings

test:
    cargo test --locked --all-targets --all-features

coverage:
    ./scripts/coverage.sh

audit-coverage:
    ./scripts/audit-coverage.sh

audit-host-dependencies:
    ./scripts/audit-host-dependencies.sh

audit-dependency-policy:
    ./scripts/audit-dependency-policy.sh

audit-docs-policy:
    ./scripts/audit-docs-policy.sh

audit-lint-policy:
    ./scripts/audit-lint-policy.sh

audit-workflow-actions:
    ./scripts/audit-workflow-actions.sh

audit-supply-chain:
    ./scripts/audit-supply-chain.sh

audit-package:
    ./scripts/audit-package.sh

audit-license-policy:
    ./scripts/audit-license-policy.sh

audit-readme-badges:
    ./scripts/audit-readme-badges.sh

audit-release-archive:
    ./scripts/audit-release-archive.sh

audit-release-workflow:
    ./scripts/audit-release-workflow.sh

audit-workflow-metadata:
    ./scripts/audit-workflow-metadata.sh

audit-readme-coverage:
    ./scripts/audit-readme-coverage.sh

audit:
    ./scripts/audit-dependency-policy.sh
    ./scripts/audit-docs-policy.sh
    ./scripts/audit-host-dependencies.sh
    ./scripts/audit-license-policy.sh
    ./scripts/audit-lint-policy.sh
    ./scripts/test-policy-audits.sh
    ./scripts/audit-package.sh
    ./scripts/audit-readme-badges.sh
    ./scripts/audit-release-archive.sh
    ./scripts/audit-workflow-actions.sh
    ./scripts/audit-release-workflow.sh
    ./scripts/audit-workflow-metadata.sh
    ./scripts/audit-supply-chain.sh
    ./scripts/audit-coverage.sh
    ./scripts/audit-readme-coverage.sh

ci:
    just lint
    just test
    just clippy
    just build
    just coverage
    just audit

fmt:
    cargo fmt
