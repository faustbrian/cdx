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
--include-subagents          Start with spawned subagent threads visible
```

## Controls

- `type` to search
- `Backspace` to delete query text
- `Ctrl+U` to clear the query
- `Esc` to clear the query first, then exit once empty
- `Tab` / `Shift+Tab` to focus search, filter, threads, or sort
- `Left` / `Right` or `Space` to change the focused filter, threads, or sort option
- `Ctrl+V` to toggle between the default `Core` columns and the `Full`
  timestamp-inclusive column set
- `Ctrl+O` to toggle between `Compact` and `Comfortable` row density
- `Up` / `Down` to move through the result list
- `Enter` to resume the selected conversation

## Default layout

`cdx` starts in a compact, reduced-column view intended for scanning:

- `Directory`
- `Conversation`
- `Excerpt`

Enable the full column set when you want `Age` and `Updated` timestamps visible.
Excerpt rendering scales to the available terminal width, and `Comfortable`
mode uses two width-aware excerpt lines to show more context per result.

The `Threads` control hides spawned subagent conversations by default and can
show them on demand. Starting `cdx --include-subagents` opens the picker with
that control already set to show all threads.

## Search behavior

Search is case-insensitive and punctuation-insensitive. For example,
`invoicerepair` will match `invoice-repair`.
