#!/usr/bin/env bash

set -euo pipefail

ROOT_DIRECTORY=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIRECTORY"
source "$ROOT_DIRECTORY/scripts/lib-search.sh"

failures=0

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
  'host package manager installation is forbidden:' \
  '\b(apt(-get)?|brew|dnf|yum|pacman|apk|zypper)\s+install\b' \
  --glob '!scripts/audit-host-dependencies.sh' \
  .github README.md INSTALLATION.md CONTRIBUTING.md SECURITY.md USAGE.md scripts justfile

report_matches \
  'language-specific global installers are forbidden outside Cargo-managed tooling:' \
  '\b(npm|pnpm|yarn|pip|pip3|poetry|composer)\s+(install|add|global)\b' \
  --glob '!scripts/audit-host-dependencies.sh' \
  .github README.md INSTALLATION.md CONTRIBUTING.md SECURITY.md USAGE.md scripts justfile

report_matches \
  'runtime service dependencies are forbidden in project automation:' \
  '\b(systemctl|service|launchctl|docker|docker compose|podman)\b' \
  --glob '!scripts/audit-host-dependencies.sh' \
  .github scripts justfile

if (( failures != 0 )); then
  exit 1
fi

printf '%s\n' 'host dependency audit passed'
