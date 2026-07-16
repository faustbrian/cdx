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
  --lib \
  --ignore-filename-regex '(.*/src/tests\.rs|.*/tests/.*|.*/src/coverage_excluded\.rs)' \
  --lcov \
  --output-path target/lcov.info

cargo llvm-cov \
  --all-features \
  --lib \
  --ignore-filename-regex '(.*/src/tests\.rs|.*/tests/.*|.*/src/coverage_excluded\.rs)' \
  --json \
  --summary-only \
  --output-path target/coverage-summary.json >/dev/null

ruby -rjson <<'RUBY' > target/coverage-summary.txt
summary = JSON.parse(File.read("target/coverage-summary.json"))
totals = summary.fetch("data").fetch(0).fetch("totals")

puts "line coverage: #{format('%.2f', totals.fetch('lines').fetch('percent'))}%"
puts "function coverage: #{format('%.2f', totals.fetch('functions').fetch('percent'))}%"
puts "region coverage: #{format('%.2f', totals.fetch('regions').fetch('percent'))}%"
puts "scope: measured library source (src/lib.rs), excluding test harness and coverage helper files"
RUBY

cat target/coverage-summary.txt

printf '%s\n' 'coverage summary completed'
