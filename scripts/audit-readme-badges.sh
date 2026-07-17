#!/usr/bin/env bash

set -euo pipefail

readonly readme_file="${1:-README.md}"

if [[ ! -f "${readme_file}" ]]; then
  printf 'README badge policy file is not a regular file: %s\n' "${readme_file}" >&2
  exit 1
fi

required_badges=(
  '[![CI](https://github.com/faustbrian/cdx/actions/workflows/ci.yml/badge.svg)](https://github.com/faustbrian/cdx/actions/workflows/ci.yml)'
  '[![Coverage Audit](https://img.shields.io/badge/coverage%20audit-enforced-brightgreen.svg)](https://github.com/faustbrian/cdx/actions/workflows/ci.yml)'
  '[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE.md)'
  '[![Scorecards](https://api.scorecard.dev/projects/github.com/faustbrian/cdx/badge)](https://scorecard.dev/viewer/?uri=github.com/faustbrian/cdx)'
)

failed=0
for badge in "${required_badges[@]}"; do
  if ! grep -Fq -- "${badge}" "${readme_file}"; then
    printf 'README is missing required badge link: %s\n' "${badge}" >&2
    failed=1
  fi
done

if (( failed != 0 )); then
  exit 1
fi

printf '%s\n' 'README badge audit passed'
