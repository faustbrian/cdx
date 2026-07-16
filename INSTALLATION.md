# Installation

`cdx` supports local builds on macOS and Linux with a recent stable Rust
toolchain. The repository intentionally avoids package-manager bootstrap steps
or service-manager setup in its documented install path.

## Build from source

```bash
git clone git@github.com:faustbrian/cdx.git
cd cdx
cargo install --path . --locked
cdx --version
```

If `just` is available:

```bash
just install
just coverage
```

## Verify a release archive

The release workflow publishes platform archives alongside SHA-256 checksum
files. After downloading a release archive and its matching `.sha256` file:

```bash
shasum -a 256 -c cdx-macos-arm64.tar.gz.sha256
tar -xzf cdx-macos-arm64.tar.gz
./cdx --version
```

The repository's strict local gate also audits the release archive shape on
the current platform:

```bash
./scripts/audit-release-archive.sh
```

## Runtime requirements

`cdx` expects a local Codex installation and its local data store under the
current user's home directory unless explicit override flags are passed.

Default paths are resolved at runtime from `$HOME`:

- `~/.codex/state_5.sqlite`
- `~/.codex/session_index.jsonl`

Override them explicitly when needed:

```bash
cdx \
  --db-path /custom/path/state_5.sqlite \
  --session-index-path /custom/path/session_index.jsonl
```
