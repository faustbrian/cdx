#!/usr/bin/env bash

set -euo pipefail

readonly manifest="${1:-Cargo.toml}"
readonly license_file="${2:-LICENSE.md}"

if [[ ! -f "${manifest}" ]]; then
  printf 'license policy manifest is not a regular file: %s\n' "${manifest}" >&2
  exit 1
fi

if [[ ! -f "${license_file}" ]]; then
  printf 'license policy file is not a regular file: %s\n' "${license_file}" >&2
  exit 1
fi

required_manifest_contract=(
  'license = "MIT"'
  'readme = "README.md"'
)

required_license_contract=(
  'MIT License'
  'Permission is hereby granted, free of charge, to any person obtaining a copy'
  'THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND'
)

failed=0

for requirement in "${required_manifest_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${manifest}"; then
    printf 'manifest is missing required license contract: %s\n' "${requirement}" >&2
    failed=1
  fi
done

for requirement in "${required_license_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${license_file}"; then
    printf 'license file is missing required MIT contract: %s\n' "${requirement}" >&2
    failed=1
  fi
done

if (( failed != 0 )); then
  exit 1
fi

printf '%s\n' 'license policy audit passed'
