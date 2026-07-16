#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"
source "$ROOT_DIRECTORY/scripts/lib-search.sh"

platform() {
  local os arch
  os=$(uname -s)
  arch=$(uname -m)

  case "$os" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    *)
      printf 'unsupported operating system: %s\n' "$os" >&2
      return 1
      ;;
  esac

  case "$arch" in
    x86_64) arch="x86_64" ;;
    arm64|aarch64) arch="arm64" ;;
    *)
      printf 'unsupported architecture: %s\n' "$arch" >&2
      return 1
      ;;
  esac

  printf '%s-%s\n' "$os" "$arch"
}

archive_name="cdx-$(platform).tar.gz"
checksum_name="${archive_name}.sha256"
staging_directory=$(mktemp -d)
extract_directory=$(mktemp -d)
archive_path="$ROOT_DIRECTORY/$archive_name"
checksum_path="$ROOT_DIRECTORY/$checksum_name"

cleanup() {
  rm -rf "$staging_directory" "$extract_directory"
  rm -f "$archive_path" "$checksum_path"
}

trap cleanup EXIT

cargo build --release --locked --all-features >/dev/null

cp target/release/cdx "$staging_directory/"
cp README.md LICENSE.md CHANGELOG.md "$staging_directory/"
tar -C "$staging_directory" -czf "$archive_path" .
shasum -a 256 "$archive_path" >"$checksum_path"
(cd "$ROOT_DIRECTORY" && shasum -a 256 -c "$checksum_name" >/dev/null)
tar -C "$extract_directory" -xzf "$archive_path"

for required_path in cdx README.md LICENSE.md CHANGELOG.md; do
  if [[ ! -e "$extract_directory/$required_path" ]]; then
    printf 'release archive is missing %s\n' "$required_path" >&2
    exit 1
  fi
done

if tar -tzf "$archive_path" | search_lines '^target/' >/dev/null; then
  printf '%s\n' 'release archive must not include target artifacts' >&2
  exit 1
fi

version_output=$("$extract_directory/cdx" --version)
if [[ "$version_output" != cdx\ * ]]; then
  printf 'unexpected version output from archived binary: %s\n' "$version_output" >&2
  exit 1
fi

printf '%s\n' 'release archive audit passed'
printf 'verified archive: %s\n' "$archive_name"
printf 'verified checksum: %s\n' "$checksum_name"
