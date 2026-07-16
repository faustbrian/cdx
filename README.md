# cdx

![CI](https://github.com/faustbrian/cdx/actions/workflows/ci.yml/badge.svg)
![Coverage](https://img.shields.io/badge/coverage-89.35%25-yellow.svg)
![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)
![Rust 2024](https://img.shields.io/badge/rust-2024-blue.svg)
![TUI](https://img.shields.io/badge/interface-terminal-informational.svg)

`cdx` is a strict local-first Rust CLI for browsing Codex conversations across
all directories on a machine and resuming them with the native `codex resume`
command.

## Features

- Global conversation index backed by the local Codex thread store.
- Search across conversation title, preview, working directory, and id.
- Search normalization that ignores punctuation, so `confirmarepair` matches
  `confirma-repair`.
- Filter conversations to the current directory or all directories.
- Sort by updated time or created time.
- Relative age plus absolute timestamps with recency coloring.
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

## Controls

- `type` to search
- `Backspace` to delete search input
- `Ctrl+U` to clear the current search
- `Esc` to clear the search, then exit when already empty
- `Tab` / `Shift+Tab` to focus search, filter, or sort
- `Left` / `Right` to change filter or sort
- `Up` / `Down` to move through results
- `Enter` to resume the selected conversation

## Development

Strict local checks:

```bash
cargo fmt --check
cargo test --all-targets --all-features
cargo build --release --locked --all-features
cargo clippy --all-targets --all-features -- -D warnings
./scripts/audit-workflow-actions.sh
./scripts/audit-supply-chain.sh
```

Additional project docs:

- [Installation](INSTALLATION.md)
- [Usage](USAGE.md)
- [Changelog](CHANGELOG.md)

Supply-chain policy:

```bash
cargo deny check
```

Current measured coverage on July 16, 2026:

- lines: `89.35%`
- functions: `87.67%`
- regions: `89.24%`

## License

`cdx` is licensed under MIT. See [LICENSE.md](LICENSE.md).
