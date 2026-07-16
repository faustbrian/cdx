#!/usr/bin/env bash

set -euo pipefail

readonly dependabot_file="${1:-.github/dependabot.yml}"
readonly dependency_review_file="${2:-.github/workflows/dependency-review.yml}"

if [[ ! -f "${dependabot_file}" ]]; then
  printf 'dependabot policy file is not a regular file: %s\n' \
    "${dependabot_file}" >&2
  exit 1
fi

if [[ ! -f "${dependency_review_file}" ]]; then
  printf 'dependency review workflow is not a regular file: %s\n' \
    "${dependency_review_file}" >&2
  exit 1
fi

dependabot_contract=(
  'version: 2'
  'package-ecosystem: cargo'
  'package-ecosystem: github-actions'
  'directory: /'
  'interval: weekly'
  'open-pull-requests-limit: 10'
  'rust-dependencies:'
  'github-actions:'
  'patterns:'
  '- "*"'
)

dependency_review_contract=(
  'pull_request:'
  'contents: read'
  'pull-requests: read'
  'actions/checkout@'
  'actions/dependency-review-action@'
  'fail-on-severity: moderate'
  'deny-licenses: GPL-2.0, GPL-3.0, AGPL-3.0'
)

failed=0
for requirement in "${dependabot_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${dependabot_file}"; then
    printf 'dependabot policy is missing required contract: %s\n' \
      "${requirement}" >&2
    failed=1
  fi
done

for requirement in "${dependency_review_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${dependency_review_file}"; then
    printf 'dependency review workflow is missing required contract: %s\n' \
      "${requirement}" >&2
    failed=1
  fi
done

if (( failed != 0 )); then
  exit 1
fi

printf '%s\n' 'dependency policy audit passed'
