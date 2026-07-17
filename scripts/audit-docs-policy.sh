#!/usr/bin/env bash

set -euo pipefail

readonly readme_file="${1:-README.md}"
readonly contributing_file="${2:-CONTRIBUTING.md}"
readonly pr_template_file="${3:-.github/pull_request_template.md}"

for file in "${readme_file}" "${contributing_file}" "${pr_template_file}"; do
  if [[ ! -f "${file}" ]]; then
    printf 'docs policy file is not a regular file: %s\n' "${file}" >&2
    exit 1
  fi
done

readme_contract=(
  'cargo fmt --check --all'
  'cargo test --locked --all-targets --all-features'
  'cargo build --release --locked --all-features'
  'cargo clippy --locked --all-targets --all-features -- -D warnings'
  './scripts/test-policy-audits.sh'
  './scripts/audit-dependency-policy.sh'
  './scripts/audit-docs-policy.sh'
  './scripts/audit-host-dependencies.sh'
  './scripts/audit-license-policy.sh'
  './scripts/audit-package.sh'
  './scripts/audit-readme-badges.sh'
  './scripts/audit-release-archive.sh'
  './scripts/audit-release-workflow.sh'
  './scripts/audit-workflow-actions.sh'
  './scripts/audit-workflow-metadata.sh'
  './scripts/audit-supply-chain.sh'
  './scripts/audit-coverage.sh'
  './scripts/audit-readme-coverage.sh'
)

contributing_contract=(
  'cargo fmt --check --all'
  'cargo test --locked --all-targets --all-features'
  'cargo build --release --locked --all-features'
  'cargo clippy --locked --all-targets --all-features -- -D warnings'
  './scripts/test-policy-audits.sh'
  './scripts/audit-dependency-policy.sh'
  './scripts/audit-docs-policy.sh'
  './scripts/audit-host-dependencies.sh'
  './scripts/audit-license-policy.sh'
  './scripts/audit-package.sh'
  './scripts/audit-readme-badges.sh'
  './scripts/audit-release-archive.sh'
  './scripts/audit-release-workflow.sh'
  './scripts/audit-readme-coverage.sh'
  './scripts/audit-workflow-actions.sh'
  './scripts/audit-workflow-metadata.sh'
  './scripts/audit-coverage.sh'
  './scripts/audit-supply-chain.sh'
  'Keep published verification docs aligned with the enforced local gate.'
)

pr_template_contract=(
  'cargo fmt --check --all'
  'cargo test --locked --all-targets --all-features'
  'cargo build --release --locked --all-features'
  'cargo clippy --locked --all-targets --all-features -- -D warnings'
  './scripts/test-policy-audits.sh'
  './scripts/audit-dependency-policy.sh'
  './scripts/audit-docs-policy.sh'
  './scripts/audit-host-dependencies.sh'
  './scripts/audit-license-policy.sh'
  './scripts/audit-package.sh'
  './scripts/audit-readme-badges.sh'
  './scripts/audit-release-archive.sh'
  './scripts/audit-release-workflow.sh'
  './scripts/audit-workflow-actions.sh'
  './scripts/audit-workflow-metadata.sh'
  './scripts/audit-supply-chain.sh'
  './scripts/audit-coverage.sh'
  './scripts/audit-readme-coverage.sh'
)

failed=0

for requirement in "${readme_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${readme_file}"; then
    printf 'README policy is missing required contract: %s\n' \
      "${requirement}" >&2
    failed=1
  fi
done

for requirement in "${contributing_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${contributing_file}"; then
    printf 'contributing policy is missing required contract: %s\n' \
      "${requirement}" >&2
    failed=1
  fi
done

for requirement in "${pr_template_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${pr_template_file}"; then
    printf 'pull request template is missing required contract: %s\n' \
      "${requirement}" >&2
    failed=1
  fi
done

if (( failed != 0 )); then
  exit 1
fi

printf '%s\n' 'docs policy audit passed'
