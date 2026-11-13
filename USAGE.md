# Usage

## Start the picker

```bash
cdx
```

## Optional flags

```text
--agent <claude|codex|all>    Agent shown on start (default: claude)
--claude-projects-path <PATH> Override the Claude Code projects directory
--claude-bin <NAME>           Override the Claude resume binary
--db-path <PATH>              Override the Codex SQLite thread store
--session-index-path <PATH>   Override the Codex session index JSONL file
--codex-bin <NAME>            Override the Codex resume binary
--dry-run                    Print the resume command instead of executing it
--allow-daemon               Omit the default Codex --no-daemon flag
--include-subagents          Start with spawned subagent threads visible
```

## Controls

Claude Code sessions are read from `~/.claude/projects` and resume with
`claude --resume <session-id>` started in the session's working directory.

Codex resumes with `--no-daemon` by default. Use `cdx --allow-daemon` to
allow Codex to use its shared background server instead. The conversation's
working directory is still passed with `-C` in either mode.

- `type` to search
- `Backspace` to delete query text
- `Ctrl+U` to clear the query
- `Esc` to clear the query first, then exit once empty
- `Tab` / `Shift+Tab` to focus search, agent, filter, threads, or sort
- `Left` / `Right` or `Space` to change the focused agent, filter, threads, or sort option
- `Ctrl+V` to toggle between the default `Core` columns and the `Full`
  timestamp-inclusive column set
- `Ctrl+O` to toggle between `Compact` and `Comfortable` row density
- `Up` / `Down` to move through the result list
- `Enter` to resume the selected conversation

## Default layout

`cdx` starts in a compact, reduced-column view intended for scanning:

- `Agent`
- `Directory`
- `Conversation`
- `Excerpt`

Enable the full column set when you want `Age` and `Updated` timestamps visible.
Excerpt rendering scales to the available terminal width, and `Comfortable`
mode uses two width-aware excerpt lines to show more context per result.

The `Agent` control switches between Claude, Codex, and all conversations and
starts on Claude. Claude rows are marked in Claude orange and highlight in
orange when selected; Codex rows are marked in blue.

The `Threads` control hides spawned subagent conversations by default and can
show them on demand. Starting `cdx --include-subagents` opens the picker with
that control already set to show all threads. For Claude, it also hides
print-mode (`claude -p`) and SDK sessions.

## Search behavior

Search is case-insensitive and punctuation-insensitive. For example,
`invoicerepair` will match `invoice-repair`.
