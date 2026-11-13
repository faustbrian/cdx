use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::claude::{SessionFile, SessionHead};
use crate::{App, Conversation, FocusTarget, ThreadRow};

pub(super) fn display_title(id: &str, thread_name: Option<&str>, title: Option<&str>) -> String {
    thread_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let trimmed_id = id.trim();
            if trimmed_id.is_empty() {
                None
            } else {
                Some(trimmed_id)
            }
        })
        .or_else(|| title.map(str::trim).filter(|value| !value.is_empty()))
        .unwrap_or("unknown")
        .to_string()
}

pub(super) fn preview_text(preview: Option<&str>, first_user_message: Option<&str>) -> String {
    let source = preview
        .or(first_user_message)
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    truncate_chars(&source, crate::PREVIEW_LIMIT).into_owned()
}

pub(super) fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn truncate_chars(value: &str, limit: usize) -> Cow<'_, str> {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(limit).collect();

    if chars.next().is_some() {
        Cow::Owned(format!("{truncated}…"))
    } else {
        Cow::Borrowed(value)
    }
}

pub(super) fn truncate_to_width(value: &str, width: usize) -> Cow<'_, str> {
    if width == 0 {
        return Cow::Borrowed("");
    }

    let char_count = value.chars().count();
    if char_count <= width {
        return Cow::Borrowed(value);
    }

    if width == 1 {
        return Cow::Borrowed("…");
    }

    let visible = value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    Cow::Owned(format!("{visible}…"))
}

pub(super) fn render_cwd_with_home(cwd: &str, home_dir: Option<&Path>) -> String {
    let Some(home_dir) = home_dir else {
        return cwd.to_string();
    };

    let developer_root = home_dir.join("Developer");
    let cwd_path = Path::new(cwd);

    let Ok(suffix) = cwd_path.strip_prefix(&developer_root) else {
        return cwd.to_string();
    };

    let suffix = suffix.display().to_string();
    if suffix.is_empty() {
        "~/Developer".to_string()
    } else {
        format!("~/Developer/{suffix}")
    }
}

pub(super) fn deduplicate_conversations(conversations: Vec<Conversation>) -> Vec<Conversation> {
    let mut seen_ids = HashSet::new();
    let mut unique = Vec::with_capacity(conversations.len());

    for conversation in conversations {
        if seen_ids.insert(conversation.id.clone()) {
            unique.push(conversation);
        }
    }

    unique
}

pub(super) fn selected_conversation<'a>(
    visible: &[usize],
    selected: usize,
    conversations: &'a [Conversation],
) -> Option<&'a Conversation> {
    visible
        .get(selected)
        .and_then(|index| conversations.get(*index))
}

pub(super) fn thread_row_is_subagent(
    row: &ThreadRow,
    subagent_thread_ids: &HashSet<String>,
) -> bool {
    let spawned_subagent = subagent_thread_ids.contains(&row.id);
    let thread_source_subagent = row.thread_source.as_deref() == Some("subagent");
    let source_mentions_subagent = row.source.as_deref().is_some_and(|source| {
        let normalized = normalize_search_text(source);
        normalized.contains("subagent")
    });

    spawned_subagent || thread_source_subagent || source_mentions_subagent
}

pub(super) fn finish_draw<T, E>(result: std::result::Result<T, E>) -> Result<()>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match result {
        Ok(_) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn restore_terminal_session<T, FDisable, FLeave, FCursor>(
    terminal: &mut T,
    disable_raw: FDisable,
    leave_alternate_screen: FLeave,
    show_cursor: FCursor,
) -> Result<()>
where
    FDisable: FnOnce() -> Result<()>,
    FLeave: FnOnce(&mut T) -> Result<()>,
    FCursor: FnOnce(&mut T) -> Result<()>,
{
    disable_raw()?;
    leave_alternate_screen(terminal)?;
    show_cursor(terminal)
}

pub(super) fn with_terminal_session<FInit, FRun, FRestore, T, R>(
    mut init: FInit,
    mut run: FRun,
    mut restore: FRestore,
) -> Result<R>
where
    FInit: FnMut() -> Result<T>,
    FRun: FnMut(&mut T) -> Result<R>,
    FRestore: FnMut(&mut T) -> Result<()>,
{
    with_terminal_session_impl(&mut init, &mut run, &mut restore)
}

fn with_terminal_session_impl<T, R>(
    init: &mut dyn FnMut() -> Result<T>,
    run: &mut dyn FnMut(&mut T) -> Result<R>,
    restore: &mut dyn FnMut(&mut T) -> Result<()>,
) -> Result<R> {
    let mut terminal = init()?;
    let result = run(&mut terminal);
    restore(&mut terminal)?;
    result
}

pub(super) fn write_dry_run_output<W: Write>(
    writer: &mut W,
    codex_bin: &str,
    conversation_id: &str,
    cwd: &str,
    allow_daemon: bool,
) -> Result<()> {
    let flag = if allow_daemon { "" } else { "--no-daemon " };
    match writeln!(
        writer,
        "{codex_bin} {flag}resume -C {cwd} {conversation_id}"
    ) {
        Ok(()) => Ok(()),
        Err(error) => Err(error).context("failed to write dry-run output"),
    }
}

pub(super) fn run_picker_loop_impl(
    app: &mut App,
    draw: &mut dyn FnMut(&App) -> Result<()>,
    poll: &mut dyn FnMut() -> Result<bool>,
    read: &mut dyn FnMut() -> Result<Event>,
) -> Result<Conversation> {
    loop {
        draw(app)?;

        if !poll()? {
            continue;
        }

        let event = read()?;
        let selected = match event {
            Event::Key(key) => handle_key_event(app, key)?,
            _ => None,
        };

        if let Some(conversation) = selected {
            return Ok(conversation);
        }
    }
}

pub(super) fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<Option<Conversation>> {
    if key.kind != KeyEventKind::Press {
        return Ok(None);
    }

    match key.code {
        KeyCode::Up => app.move_up(),
        KeyCode::Down => app.move_down(),
        KeyCode::Left => app.change_current_option_left(),
        KeyCode::Right => app.change_current_option_right(),
        KeyCode::Char(' ') if app.focus != FocusTarget::Search => app.change_current_option_right(),
        KeyCode::Tab => app.cycle_focus_forward(),
        KeyCode::BackTab => app.cycle_focus_backward(),
        KeyCode::Backspace => app.backspace_query(),
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_row_mode();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.clear_query();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_column_preset();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            bail!("selection cancelled");
        }
        KeyCode::Esc => {
            if app.escape() {
                bail!("selection cancelled");
            }
        }
        KeyCode::Enter => {
            if let Some(conversation) = app.selected_conversation() {
                return Ok(Some(conversation.clone()));
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.append_query(ch);
        }
        _ => {}
    }

    Ok(None)
}

/// Reads the head of every `<projects>/<project>/<session>.jsonl` file.
///
/// A missing projects directory yields no sessions, so machines without
/// Claude Code still show their Codex history.
pub(super) fn read_session_files(
    projects_path: &Path,
    head_bytes: u64,
    tail_bytes: u64,
) -> Result<Vec<SessionFile>> {
    let project_dirs = match fs::read_dir(projects_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).context(format!("failed to read {}", projects_path.display()));
        }
    };

    let mut sessions = Vec::new();
    for project_dir in project_dirs {
        let project_dir = project_dir
            .context(format!("failed to read {}", projects_path.display()))?
            .path();
        if !project_dir.is_dir() {
            continue;
        }

        for path in jsonl_files(&project_dir)? {
            sessions.extend(read_session_file(&path, head_bytes, tail_bytes)?);
        }
    }

    Ok(sessions)
}

fn jsonl_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).context(format!("failed to read {}", dir.display()))? {
        let path = entry
            .context(format!("failed to read {}", dir.display()))?
            .path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            paths.push(path);
        }
    }

    Ok(paths)
}

/// Returns `None` for a session deleted after its directory was listed, which
/// Claude Code does when it cleans up old transcripts on startup.
fn read_session_file(path: &Path, head_bytes: u64, tail_bytes: u64) -> Result<Option<SessionFile>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context(format!("failed to open {}", path.display())),
    };
    let updated_at_ms = modified_at_ms(&file).context(format!(
        "failed to read modification time of {}",
        path.display()
    ))?;
    let mut head = read_session_head(BufReader::new((&file).take(head_bytes)))
        .context(format!("failed to read {}", path.display()))?;
    read_session_tail(&file, tail_bytes, &mut head)
        .context(format!("failed to read {}", path.display()))?;

    Ok(Some(SessionFile {
        id: path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
        head,
        updated_at_ms,
    }))
}

fn read_session_tail(file: &File, tail_bytes: u64, head: &mut SessionHead) -> io::Result<()> {
    let length = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(length.saturating_sub(tail_bytes)))?;

    // The seek usually lands mid-record; that first fragment fails to parse
    // and is skipped like any other unusable line.
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        head.absorb_tail_line(&line);
    }
}

fn modified_at_ms(file: &File) -> io::Result<i64> {
    let modified = file.metadata()?.modified()?;
    let since_epoch = modified
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;

    i64::try_from(since_epoch.as_millis()).map_err(io::Error::other)
}

pub(super) fn read_session_head<R: BufRead>(mut reader: R) -> io::Result<SessionHead> {
    let mut head = SessionHead::default();
    let mut line = Vec::new();

    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || head.absorb_line(&line) {
            return Ok(head);
        }
    }
}
