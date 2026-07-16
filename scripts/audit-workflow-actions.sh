#!/usr/bin/env bash

set -euo pipefail

readonly workflow_root="${1:-.github/workflows}"

if [[ ! -d "${workflow_root}" ]]; then
  printf 'workflow action audit root is not a directory: %s\n' \
    "${workflow_root}" >&2
  exit 1
fi

failed=0
while IFS=: read -r file line_number declaration; do
  reference="${declaration#*uses:}"
  reference="${reference#"${reference%%[![:space:]]*}"}"

  if [[ "${reference}" == ./* ]]; then
    continue
  fi

  revision="${reference##*@}"
  if [[ ! "${revision}" =~ ^[0-9a-f]{40}$ ]]; then
    printf '%s:%s: workflow action must use an immutable 40-character commit: %s\n' \
      "${file}" "${line_number}" "${reference}" >&2
    failed=1
  fi
done < <(rg --no-heading --line-number '^[[:space:]]*uses:' "${workflow_root}")

if (( failed != 0 )); then
  exit 1
fi

printf '%s\n' 'workflow action audit passed'
