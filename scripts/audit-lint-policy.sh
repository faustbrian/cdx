#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"
source "$ROOT_DIRECTORY/scripts/lib-search.sh"

failures=0

required_clippy_lints=(
  'unwrap_used = "deny"'
  'expect_used = "deny"'
  'panic = "deny"'
  'unimplemented = "deny"'
  'todo = "deny"'
  'unwrap_in_result = "deny"'
  'exit = "deny"'
  'mem_forget = "deny"'
  'dbg_macro = "deny"'
  'print_stdout = "deny"'
  'print_stderr = "deny"'
  'large_include_file = "deny"'
  'infinite_loop = "deny"'
  'lossy_float_literal = "deny"'
  'tests_outside_test_module = "deny"'
  'try_err = "deny"'
)

if ! search_file_quiet '^unused = \{ level = "deny", priority = -1 \}$' Cargo.toml; then
  printf '%s\n' 'Cargo.toml must deny unused production and test code' >&2
  failures=1
fi

for lint_rule in "${required_clippy_lints[@]}"; do
  if ! search_file_quiet "^${lint_rule}$" Cargo.toml; then
    printf 'Cargo.toml must enforce strict Clippy lint: %s\n' "${lint_rule}" >&2
    failures=1
  fi
done

report_matches() {
  local description=$1
  local pattern=$2
  shift 2

  local matches
  if matches=$(search_lines "$pattern" "$@"); then
    printf '%s\n%s\n' "$description" "$matches" >&2
    failures=1
  fi
}

report_matches \
  'crate-wide compiler warning suppression is forbidden:' \
  '#!\[allow\([^]]*(dead_code|unused_imports)' \
  src

report_matches \
  'broad Clippy group suppression is forbidden:' \
  '#!\[allow\([^]]*clippy::(all|pedantic|nursery|cargo)' \
  src

report_matches \
  'build and run recipes must not disable compiler warnings:' \
  'RUSTFLAGS=.*-A ?warnings|RUSTFLAGS=.*--allow[= ]warnings' \
  justfile scripts .github

if (( failures != 0 )); then
  exit 1
fi

printf '%s\n' 'lint policy audit passed'
