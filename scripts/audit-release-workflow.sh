#!/usr/bin/env bash

set -euo pipefail

readonly workflow="${1:-.github/workflows/release.yml}"

if [[ ! -f "${workflow}" ]]; then
  printf 'release workflow is not a regular file: %s\n' "${workflow}" >&2
  exit 1
fi

required_contract=(
  'workflow_dispatch:'
  'group: release-${{ github.ref }}'
  'Validate release identity'
  'GITHUB_REF_NAME'
  'v${version}'
  'x86_64-unknown-linux-gnu'
  'aarch64-apple-darwin'
  './scripts/audit-package.sh'
  './scripts/audit-release-archive.sh'
  'cargo build --release --locked --all-features --target'
  'anchore/sbom-action@'
  'actions/attest-build-provenance@'
  'actions/attest-sbom@'
  'gh attestation verify'
  '--source-digest "${GITHUB_SHA}"'
  'https://slsa.dev/provenance/v1'
  'https://spdx.dev/Document/v2.3'
  'SHA256SUMS'
  'gh release create'
  'verified-release-evidence'
)

failed=0
for requirement in "${required_contract[@]}"; do
  if ! grep -Fq -- "${requirement}" "${workflow}"; then
    printf 'release workflow is missing required contract: %s\n' \
      "${requirement}" >&2
    failed=1
  fi
done

if (( failed != 0 )); then
  exit 1
fi

printf '%s\n' 'release workflow audit passed'
