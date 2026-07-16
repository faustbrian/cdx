# Changelog

All notable changes to `cdx` will be documented in this file.

## Unreleased

- Initial global Codex conversation picker implementation.
- Search, filter, sort, and recency-aware terminal UI.
- Search-bar controls now reserve safe width so sort and filter labels do not
  clip at narrow terminal sizes.
- Excerpt rendering now scales to the live table width, with comfortable rows
  using two width-aware excerpt lines instead of a fixed character cap.
- Runtime thread visibility toggle for hiding or showing spawned subagent
  conversations without restarting the picker.
- Session-name preference via `session_index.jsonl`.
- Explicit conversation-id deduplication in the loader.
- Strict package metadata, MIT licensing, CI, and supply-chain policy.
- Dependabot and dependency-review policy are now audited alongside the rest of
  the repository governance gates.
- README, contributing guidance, and the PR template are now audited against
  the enforced local verification contract.
- GitHub release, dependency-review, and scorecard workflows.
- Measured library-surface coverage audit plus binary/library split for cleaner
  terminal-entrypoint packaging.
- Publish-facing documentation and security policy scaffolding.
