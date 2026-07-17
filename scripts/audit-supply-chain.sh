#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"

if ! command -v cargo-audit >/dev/null 2>&1; then
  printf '%s\n' 'cargo-audit is required for the supply-chain audit' >&2
  exit 69
fi

if ! command -v cargo-deny >/dev/null 2>&1; then
  printf '%s\n' 'cargo-deny is required for the supply-chain audit' >&2
  exit 69
fi

cargo audit --deny warnings
cargo deny check -D warnings advisories bans licenses sources

printf '%s\n' 'supply-chain audit passed'
