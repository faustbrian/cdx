#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"
source "$ROOT_DIRECTORY/scripts/lib-search.sh"

TEMP_ROOT=$(mktemp -d)
trap 'rm -rf "${TEMP_ROOT}"' EXIT INT TERM

mkdir -p "${TEMP_ROOT}/valid" "${TEMP_ROOT}/mutable" "${TEMP_ROOT}/empty"
mkdir -p "${TEMP_ROOT}/release"
mkdir -p "${TEMP_ROOT}/metadata/valid" "${TEMP_ROOT}/metadata/invalid"
mkdir -p "${TEMP_ROOT}/dependency-policy"
mkdir -p "${TEMP_ROOT}/docs-policy"
mkdir -p "${TEMP_ROOT}/license-policy"
mkdir -p "${TEMP_ROOT}/badge-policy"

cat >"${TEMP_ROOT}/valid/ci.yml" <<'YAML'
steps:
  - name: Checkout
    uses: actions/checkout@93cb6efe18208431cddfb8368fd83d5badbf9bfd
  - name: Local action
    uses: ./local-action
YAML

cat >"${TEMP_ROOT}/mutable/ci.yml" <<'YAML'
steps:
  - name: Checkout
    uses: actions/checkout@main
YAML

cat >"${TEMP_ROOT}/release/valid.yml" <<'YAML'
on:
  workflow_dispatch:
concurrency:
  group: release-${{ github.ref }}
jobs:
  build-release:
    timeout-minutes: 20
    permissions:
      attestations: write
      contents: read
      id-token: write
    steps:
      - name: Validate release identity
        run: |
          if [[ "${GITHUB_REF_NAME}" != "v${version}" ]]; then
            exit 1
          fi
      - name: Package audit
        run: ./scripts/audit-package.sh
      - name: Release archive audit
        run: ./scripts/audit-release-archive.sh
      - name: Build
        run: cargo build --release --locked --all-features --target x86_64-unknown-linux-gnu
      - name: Build macOS
        run: cargo build --release --locked --all-features --target aarch64-apple-darwin
      - name: SBOM
        uses: anchore/sbom-action@e22c389904149dbc22b58101806040fa8d37a610
      - name: Provenance
        uses: actions/attest-build-provenance@977bb373ede98d70efdf65b84cb5f73e068dcc2a
      - name: SBOM attestation
        uses: actions/attest-sbom@4651f806c01d8637787e274ac3bdf724ef169f34
  verify-and-publish:
    timeout-minutes: 20
    permissions:
      actions: read
      attestations: read
      contents: write
    steps:
      - name: Verify
        run: |
          gh attestation verify release.tar.gz --source-digest "${GITHUB_SHA}" --predicate-type 'https://slsa.dev/provenance/v1'
          gh attestation verify release.tar.gz --source-digest "${GITHUB_SHA}" --predicate-type 'https://spdx.dev/Document/v2.3'
          sha256sum --check SHA256SUMS
      - name: Publish
        run: gh release create "${GITHUB_REF_NAME}" dist/*.tar.gz dist/SHA256SUMS
      - name: Evidence
        run: ls verified-release-evidence
YAML

cat >"${TEMP_ROOT}/release/invalid.yml" <<'YAML'
jobs:
  build-release:
    steps:
      - name: Publish
        uses: softprops/action-gh-release@da05d552573ad5aba039eaac05058a918a7bf631
YAML

cat >"${TEMP_ROOT}/metadata/valid/ci.yml" <<'YAML'
permissions:
  contents: read
jobs:
  check:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: true
YAML

cat >"${TEMP_ROOT}/metadata/invalid/ci.yml" <<'YAML'
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - run: true
YAML

cat >"${TEMP_ROOT}/dependency-policy/dependabot.yml" <<'YAML'
version: 2
updates:
  - package-ecosystem: cargo
    directory: /
    schedule:
      interval: weekly
    open-pull-requests-limit: 10
    groups:
      rust-dependencies:
        patterns:
          - "*"

  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
    open-pull-requests-limit: 10
    groups:
      github-actions:
        patterns:
          - "*"
YAML

cat >"${TEMP_ROOT}/dependency-policy/dependency-review.yml" <<'YAML'
name: Dependency Review

on:
  pull_request:

permissions:
  contents: read
  pull-requests: read

jobs:
  dependency-review:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@93cb6efe18208431cddfb8368fd83d5badbf9bfd
      - uses: actions/dependency-review-action@4d0f94946f703b95b768976c00d48484d31a1a7f
        with:
          fail-on-severity: moderate
          deny-licenses: GPL-2.0, GPL-3.0, AGPL-3.0
YAML

cat >"${TEMP_ROOT}/dependency-policy/dependabot-invalid.yml" <<'YAML'
version: 2
updates:
  - package-ecosystem: cargo
    directory: /
    schedule:
      interval: monthly
YAML

cat >"${TEMP_ROOT}/dependency-policy/dependency-review-invalid.yml" <<'YAML'
name: Dependency Review

on:
  pull_request:

permissions:
  contents: read

jobs:
  dependency-review:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/dependency-review-action@4d0f94946f703b95b768976c00d48484d31a1a7f
        with:
          fail-on-severity: low
YAML

cat >"${TEMP_ROOT}/docs-policy/README.md" <<'MARKDOWN'
# cdx

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
./scripts/audit-workflow-actions.sh
./scripts/audit-workflow-metadata.sh
./scripts/audit-supply-chain.sh
./scripts/audit-coverage.sh
./scripts/audit-readme-coverage.sh
```
MARKDOWN

cat >"${TEMP_ROOT}/docs-policy/CONTRIBUTING.md" <<'MARKDOWN'
# Contributing

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

- Keep published verification docs aligned with the enforced local gate.
MARKDOWN

cat >"${TEMP_ROOT}/docs-policy/pull_request_template.md" <<'MARKDOWN'
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
MARKDOWN

cat >"${TEMP_ROOT}/license-policy/Cargo.toml" <<'TOML'
[package]
name = "cdx"
version = "0.1.0"
license = "MIT"
readme = "README.md"
TOML

cat >"${TEMP_ROOT}/license-policy/LICENSE.md" <<'MARKDOWN'
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.
MARKDOWN

cat >"${TEMP_ROOT}/license-policy/Cargo-invalid.toml" <<'TOML'
[package]
name = "cdx"
version = "0.1.0"
license = "Apache-2.0"
TOML

cat >"${TEMP_ROOT}/license-policy/LICENSE-invalid.md" <<'MARKDOWN'
Custom License
MARKDOWN

cat >"${TEMP_ROOT}/badge-policy/README.md" <<'MARKDOWN'
[![CI](https://github.com/faustbrian/cdx/actions/workflows/ci.yml/badge.svg)](https://github.com/faustbrian/cdx/actions/workflows/ci.yml)
[![Coverage Audit](https://img.shields.io/badge/coverage%20audit-enforced-brightgreen.svg)](https://github.com/faustbrian/cdx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE.md)
[![Scorecards](https://api.scorecard.dev/projects/github.com/faustbrian/cdx/badge)](https://scorecard.dev/viewer/?uri=github.com/faustbrian/cdx)
MARKDOWN

cat >"${TEMP_ROOT}/badge-policy/README-invalid.md" <<'MARKDOWN'
![CI](https://github.com/faustbrian/cdx/actions/workflows/ci.yml/badge.svg)
MARKDOWN

cat >"${TEMP_ROOT}/docs-policy/README-invalid.md" <<'MARKDOWN'
# cdx

```bash
cargo fmt --check
cargo test --all-targets --all-features
```
MARKDOWN

PATH="/usr/bin:/bin" \
  ./scripts/audit-workflow-actions.sh "${TEMP_ROOT}/valid"

PATH="/usr/bin:/bin" \
  ./scripts/audit-release-workflow.sh "${TEMP_ROOT}/release/valid.yml"

PATH="/usr/bin:/bin" \
  ./scripts/audit-workflow-metadata.sh "${TEMP_ROOT}/metadata/valid"

PATH="/usr/bin:/bin" \
  ./scripts/audit-dependency-policy.sh \
  "${TEMP_ROOT}/dependency-policy/dependabot.yml" \
  "${TEMP_ROOT}/dependency-policy/dependency-review.yml"

PATH="/usr/bin:/bin" \
  ./scripts/audit-docs-policy.sh \
  "${TEMP_ROOT}/docs-policy/README.md" \
  "${TEMP_ROOT}/docs-policy/CONTRIBUTING.md" \
  "${TEMP_ROOT}/docs-policy/pull_request_template.md"

PATH="/usr/bin:/bin" \
  ./scripts/audit-license-policy.sh \
  "${TEMP_ROOT}/license-policy/Cargo.toml" \
  "${TEMP_ROOT}/license-policy/LICENSE.md"

PATH="/usr/bin:/bin" \
  ./scripts/audit-readme-badges.sh \
  "${TEMP_ROOT}/badge-policy/README.md"

if PATH="/usr/bin:/bin" ./scripts/audit-workflow-actions.sh \
  "${TEMP_ROOT}/mutable" >/dev/null 2>&1; then
  printf '%s\n' 'workflow action audit accepted a mutable reference' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-workflow-actions.sh \
  "${TEMP_ROOT}/empty" >/dev/null 2>&1; then
  printf '%s\n' 'workflow action audit accepted an empty scan' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-release-workflow.sh \
  "${TEMP_ROOT}/release/invalid.yml" >/dev/null 2>&1; then
  printf '%s\n' 'release workflow audit accepted a weakened contract' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-workflow-metadata.sh \
  "${TEMP_ROOT}/metadata/invalid" >/dev/null 2>&1; then
  printf '%s\n' 'workflow metadata audit accepted missing permissions or timeouts' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-dependency-policy.sh \
  "${TEMP_ROOT}/dependency-policy/dependabot-invalid.yml" \
  "${TEMP_ROOT}/dependency-policy/dependency-review.yml" >/dev/null 2>&1; then
  printf '%s\n' 'dependency policy audit accepted a weakened dependabot policy' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-dependency-policy.sh \
  "${TEMP_ROOT}/dependency-policy/dependabot.yml" \
  "${TEMP_ROOT}/dependency-policy/dependency-review-invalid.yml" >/dev/null 2>&1; then
  printf '%s\n' 'dependency policy audit accepted a weakened dependency review policy' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-docs-policy.sh \
  "${TEMP_ROOT}/docs-policy/README-invalid.md" \
  "${TEMP_ROOT}/docs-policy/CONTRIBUTING.md" \
  "${TEMP_ROOT}/docs-policy/pull_request_template.md" >/dev/null 2>&1; then
  printf '%s\n' 'docs policy audit accepted weakened published verification docs' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-license-policy.sh \
  "${TEMP_ROOT}/license-policy/Cargo-invalid.toml" \
  "${TEMP_ROOT}/license-policy/LICENSE.md" >/dev/null 2>&1; then
  printf '%s\n' 'license policy audit accepted a weakened manifest contract' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-license-policy.sh \
  "${TEMP_ROOT}/license-policy/Cargo.toml" \
  "${TEMP_ROOT}/license-policy/LICENSE-invalid.md" >/dev/null 2>&1; then
  printf '%s\n' 'license policy audit accepted a weakened license file contract' >&2
  exit 1
fi

if PATH="/usr/bin:/bin" ./scripts/audit-readme-badges.sh \
  "${TEMP_ROOT}/badge-policy/README-invalid.md" >/dev/null 2>&1; then
  printf '%s\n' 'README badge audit accepted a weakened badge contract' >&2
  exit 1
fi

PATH="/usr/bin:/bin" ./scripts/audit-lint-policy.sh

cp Cargo.toml "${TEMP_ROOT}/Cargo.toml.backup"
python3 - <<'PY'
from pathlib import Path

path = Path("Cargo.toml")
text = path.read_text()
target = 'unwrap_in_result = "deny"\n'
if target not in text:
    raise SystemExit("expected strict clippy lint rule to exist")
path.write_text(text.replace(target, "", 1))
PY

if PATH="/usr/bin:/bin" ./scripts/audit-lint-policy.sh \
  >/dev/null 2>&1; then
  printf '%s\n' 'lint policy audit accepted a missing strict Clippy rule' >&2
  mv "${TEMP_ROOT}/Cargo.toml.backup" Cargo.toml
  exit 1
fi

mv "${TEMP_ROOT}/Cargo.toml.backup" Cargo.toml

printf '%s\n' 'policy audit regression tests passed'
