# Installation

`cdx` supports local builds on macOS and Linux with a recent stable Rust
toolchain.

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
