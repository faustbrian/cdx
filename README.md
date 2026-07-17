# cdx

[![CI](https://github.com/faustbrian/cdx/actions/workflows/ci.yml/badge.svg)](https://github.com/faustbrian/cdx/actions/workflows/ci.yml)
[![Coverage Audit](https://img.shields.io/badge/coverage%20audit-enforced-brightgreen.svg)](https://github.com/faustbrian/cdx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE.md)
[![Rust 2024](https://img.shields.io/badge/rust-2024-blue.svg)](Cargo.toml)
[![TUI](https://img.shields.io/badge/interface-terminal-informational.svg)](USAGE.md)
[![Scorecards](https://api.scorecard.dev/projects/github.com/faustbrian/cdx/badge)](https://scorecard.dev/viewer/?uri=github.com/faustbrian/cdx)

`cdx` is a strict local-first Rust CLI for browsing Codex conversations across
all directories on a machine and resuming them with the native `codex resume`
command.

## Features

- Global conversation index backed by the local Codex thread store.
- Search across conversation title, preview, working directory, and id.
- Search normalization that ignores punctuation, so `invoicerepair` matches
  `invoice-repair`.
- Filter conversations to the current directory or all directories.
- Toggle spawned subagent conversations in the UI without restarting.
- Sort by updated time or created time.
- Core-by-default columns focused on directory, conversation, and excerpt, with
  an optional full timestamp view.
- Compact and comfortable row density modes for either tighter scanning or more
  context per result.
- Width-aware excerpt rendering that uses more of the terminal when space is
  available, with `Comfortable` mode showing two excerpt lines.
- Relative age plus absolute timestamps with recency coloring when the full
  column set is enabled.
- Spawned subagent conversations are hidden by default to keep the picker
  focused on user-visible threads, with an explicit opt-in when needed.
- Policy audit scripts are self-tested so CI proves mutable workflow pins and
  weakened lint policy are rejected.
- Safe resume handoff through `codex resume <conversation-id>`.

## Installation

Install from the current checkout:

```bash
cargo install --path . --locked
cdx --version
```

Or, if `just` is available:

```bash
just install
```

## Usage

Run the picker:

```bash
cdx
```

Optional overrides:

```bash
cdx --db-path ~/.codex/state_5.sqlite \
    --session-index-path ~/.codex/session_index.jsonl \
    --codex-bin codex
```

Start with spawned subagent threads already visible when you explicitly want
to inspect them:

```bash
cdx --include-subagents
```

## Controls

- `type` to search
- `Backspace` to delete search input
- `Ctrl+U` to clear the current search
- `Esc` to clear the search, then exit when already empty
- `Tab` / `Shift+Tab` to focus search, filter, threads, or sort
- `Left` / `Right` or `Space` to change the focused filter, threads, or sort option
- `Ctrl+V` to toggle between `Core` and `Full` columns
- `Ctrl+O` to toggle between `Compact` and `Comfortable` row modes
- `Up` / `Down` to move through results
- `Enter` to resume the selected conversation

## Development

Strict local checks:

```bash
cargo fmt --check --all
cargo test --locked --all-targets --all-features
cargo build --release --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
./scripts/test-policy-audits.sh
./scripts/audit-dependency-policy.sh
./scripts/audit-docs-policy.sh
./scripts/audit-host-dependencies.sh
./scripts/audit-license-policy.sh
./scripts/audit-package.sh
./scripts/audit-readme-badges.sh
./scripts/audit-release-archive.sh
./scripts/audit-release-workflow.sh
./scripts/audit-workflow-actions.sh
./scripts/audit-workflow-metadata.sh
./scripts/audit-supply-chain.sh
./scripts/audit-coverage.sh
./scripts/audit-readme-coverage.sh
```

Additional project docs:

- [Installation](INSTALLATION.md)
- [Usage](USAGE.md)
- [Changelog](CHANGELOG.md)

Supply-chain policy:

```bash
cargo deny check
```

Release artifact policy:

- release archives include `cdx`, `README.md`, `LICENSE.md`, and
  `CHANGELOG.md`
- every release archive ships with a SHA-256 checksum file
- every release archive ships with an SPDX SBOM and Sigstore attestation
- release publication verifies checksums, provenance attestations, and SBOM
  attestations against the exact published source digest before creating the
  GitHub release
- `./scripts/audit-release-archive.sh` verifies the archive contents,
  checksum, and extracted binary on the current platform

Current measured coverage from the latest local `./scripts/audit-coverage.sh`
run for the measured library surface in `src/lib.rs` (excluding
`src/tests.rs`, `tests/`, the coverage helper module in
`src/coverage_excluded.rs`, and the binary wrapper in `src/main.rs`):

- lines: `100.00%`
- functions: `100.00%`
- regions: `100.00%`

The enforced coverage audit now passes at those measurements against the
current `100.00%` floors for lines, functions, and regions. The binary
entrypoint in `src/main.rs` is intentionally kept thin, but it is not measured
by the current library-only `cargo llvm-cov` gate. The helper module
`src/coverage_excluded.rs` contains coverage-hostile terminal and bookkeeping
paths and is intentionally excluded from the measured scope.

## License

`cdx` is licensed under MIT. See [LICENSE.md](LICENSE.md).
