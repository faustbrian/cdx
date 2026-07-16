#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  printf '%s\n' 'cargo-llvm-cov is required for the coverage audit' >&2
  exit 69
fi

cargo llvm-cov \
  --all-features \
  --all-targets \
  --lcov \
  --output-path target/lcov.info \
  --summary-only | tee target/coverage-summary.txt

printf '%s\n' 'coverage summary completed'
