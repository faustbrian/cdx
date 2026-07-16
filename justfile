run: fmt
    cargo run --release

run-dev: fmt
    cargo run

install:
    cargo install --path . --locked

build: fmt
    cargo build --release --locked

build-dev: fmt
    cargo build

lint:
    cargo fmt --check

clippy:
    cargo clippy --locked --all-targets --all-features -- -D warnings

test:
    cargo test --locked --all-targets --all-features

coverage:
    ./scripts/coverage.sh

audit:
    ./scripts/audit-lint-policy.sh
    ./scripts/audit-workflow-actions.sh
    ./scripts/audit-supply-chain.sh

ci:
    just fmt
    just test
    just clippy
    just build
    just audit

fmt:
    cargo fmt
