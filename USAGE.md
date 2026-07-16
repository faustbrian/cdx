# Usage

## Start the picker

```bash
cdx
```

## Optional flags

```text
--db-path <PATH>             Override the Codex SQLite thread store
--session-index-path <PATH>  Override the Codex session index JSONL file
--codex-bin <NAME>           Override the resume command binary
--dry-run                    Print the resume command instead of executing it
```

## Controls

- `type` to search
- `Backspace` to delete query text
- `Ctrl+U` to clear the query
- `Esc` to clear the query first, then exit once empty
- `Tab` / `Shift+Tab` to focus search, filter, or sort
- `Left` / `Right` to change filter or sort
- `Up` / `Down` to move through the result list
- `Enter` to resume the selected conversation

## Search behavior

Search is case-insensitive and punctuation-insensitive. For example,
`confirmarepair` will match `confirma-repair`.
