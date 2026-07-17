#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"
source "$ROOT_DIRECTORY/scripts/lib-search.sh"

package_listing=$(mktemp)
trap 'rm -f "$package_listing"' EXIT

cargo package --allow-dirty --locked --list >"$package_listing"
cargo package --allow-dirty --locked >/dev/null

if search_file_lines '^target/' "$package_listing" >/dev/null; then
  printf '%s\n' 'packaged crate must not include target artifacts' >&2
  exit 1
fi

if ! search_file_exact_quiet 'Cargo.toml' "$package_listing"; then
  printf '%s\n' 'packaged crate must include Cargo.toml' >&2
  exit 1
fi

if ! search_file_exact_quiet 'README.md' "$package_listing"; then
  printf '%s\n' 'packaged crate must include README.md' >&2
  exit 1
fi

if ! search_file_exact_quiet 'LICENSE.md' "$package_listing"; then
  printf '%s\n' 'packaged crate must include LICENSE.md' >&2
  exit 1
fi

for documentation_path in CHANGELOG.md INSTALLATION.md USAGE.md SECURITY.md CONTRIBUTING.md; do
  if ! search_file_exact_quiet "${documentation_path}" "$package_listing"; then
    printf 'packaged crate must include %s\n' "${documentation_path}" >&2
    exit 1
  fi
done

if ! search_file_exact_quiet 'src/lib.rs' "$package_listing"; then
  printf '%s\n' 'packaged crate must include src/lib.rs' >&2
  exit 1
fi

if ! search_file_exact_quiet 'src/main.rs' "$package_listing"; then
  printf '%s\n' 'packaged crate must include src/main.rs' >&2
  exit 1
fi

if ! search_file_quiet '^tests/.+\.rs$' "$package_listing"; then
  printf '%s\n' 'packaged crate must include CLI or library regression tests' >&2
  exit 1
fi

printf '%s\n' 'package audit passed'
