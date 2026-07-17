//! Terminal UI for browsing and resuming local Codex conversations.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(not(test))]
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local, TimeDelta, TimeZone, Utc};
use crossterm::event::Event;
#[cfg(test)]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(not(test))]
use crossterm::execute;
#[cfg(not(test))]
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
#[cfg(not(test))]
use ratatui::backend::CrosstermBackend;
#[cfg(test)]
use ratatui::backend::TestBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState,
};
use rusqlite::Connection;
use serde::Deserialize;

mod coverage_excluded;

use coverage_excluded::{
    deduplicate_conversations, display_title, finish_draw, normalize_search_text, preview_text,
    render_cwd_with_home, restore_terminal_session, run_picker_loop_impl,
    selected_conversation as pick_selected_conversation, thread_row_is_subagent, truncate_chars,
    truncate_to_width, with_terminal_session, write_dry_run_output,
};

const PREVIEW_LIMIT: usize = 120;
const MIN_SEARCH_WIDTH: u16 = 18;
const CONTROLS_SAFE_MARGIN: u16 = 2;
const TABLE_COLUMN_SPACING: u16 = 1;
const TABLE_HIGHLIGHT_WIDTH: u16 = 2;
const TABLE_SCROLLBAR_WIDTH: u16 = 1;
const CHILD_THREAD_IDS_QUERY: &str =
    "select coalesce(json_group_array(child_thread_id), '[]') from thread_spawn_edges";
/// The input polling interval used by the interactive terminal picker.
pub const POLL_INTERVAL_MS: u64 = 250;

/// Runtime configuration for the interactive CLI flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Optional override for the Codex `SQLite` thread store.
    pub db_path: Option<PathBuf>,
    /// Optional override for the Codex session index JSONL file.
    pub session_index_path: Option<PathBuf>,
    /// The binary used for `codex resume`.
    pub codex_bin: String,
    /// Prints the resume command instead of executing it.
    pub dry_run: bool,
    /// Includes spawned subagent conversations in the picker.
    pub include_subagents: bool,
}

/// A local Codex conversation row rendered in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    id: String,
    cwd: String,
    rendered_cwd: String,
    display_title: String,
    preview: String,
    updated_at_ms: i64,
    created_at_ms: i64,
    is_subagent: bool,
}

#[derive(Debug)]
struct ThreadRow {
    id: String,
    cwd: String,
    title: Option<String>,
    preview: Option<String>,
    first_user_message: Option<String>,
    recency_at_ms: i64,
    updated_at_ms: i64,
    created_at_ms: i64,
    source: Option<String>,
    thread_source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionIndexEntry {
    id: String,
    thread_name: Option<String>,
}

#[derive(Debug)]
struct ThreadQuerySupport {
    subagent_thread_ids: HashSet<String>,
    source_select: &'static str,
    thread_source_select: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterMode {
    CurrentDirectory,
    All,
}

impl FilterMode {
    fn next(self) -> Self {
        match self {
            Self::CurrentDirectory => Self::All,
            Self::All => Self::CurrentDirectory,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }

    fn label(self) -> &'static str {
        match self {
            Self::CurrentDirectory => "[Cwd] All",
            Self::All => "Cwd [All]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubagentMode {
    Hide,
    Show,
}

impl SubagentMode {
    fn next(self) -> Self {
        match self {
            Self::Hide => Self::Show,
            Self::Show => Self::Hide,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hide => "[Primary] All",
            Self::Show => "Primary [All]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Updated,
    Created,
}

impl SortMode {
    fn next(self) -> Self {
        match self {
            Self::Updated => Self::Created,
            Self::Created => Self::Updated,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Updated => "[Updated] Created",
            Self::Created => "Updated [Created]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Search,
    Filter,
    Subagents,
    Sort,
}

impl FocusTarget {
    fn next(self) -> Self {
        match self {
            Self::Search => Self::Filter,
            Self::Filter => Self::Subagents,
            Self::Subagents => Self::Sort,
            Self::Sort => Self::Search,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Search => Self::Sort,
            Self::Filter => Self::Search,
            Self::Subagents => Self::Filter,
            Self::Sort => Self::Subagents,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnPreset {
    Core,
    Full,
}

impl ColumnPreset {
    fn toggle(self) -> Self {
        match self {
            Self::Core => Self::Full,
            Self::Full => Self::Core,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Full => "Full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowMode {
    Compact,
    Comfortable,
}

impl RowMode {
    fn toggle(self) -> Self {
        match self {
            Self::Compact => Self::Comfortable,
            Self::Comfortable => Self::Compact,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecencyBucket {
    Recent,
    ThisWeek,
    Stale,
}

impl RecencyBucket {
    fn style(self) -> Style {
        match self {
            Self::Recent => Style::default().fg(Color::Cyan),
            Self::ThisWeek => Style::default().fg(Color::Yellow),
            Self::Stale => Style::default().fg(Color::LightRed),
        }
    }
}

/// Interactive picker state for browsing local Codex conversations.
#[derive(Debug)]
pub struct App {
    conversations: Vec<Conversation>,
    visible: Vec<usize>,
    selected: usize,
    query: String,
    filter_mode: FilterMode,
    sort_mode: SortMode,
    focus: FocusTarget,
    current_cwd: String,
    column_preset: ColumnPreset,
    row_mode: RowMode,
    subagent_mode: SubagentMode,
}

impl App {
    /// Creates a picker state for the provided conversations and current directory.
    #[must_use]
    pub fn new(conversations: Vec<Conversation>, current_cwd: String) -> Self {
        Self::new_with_options(conversations, current_cwd, false)
    }

    /// Creates a picker state with an explicit initial subagent visibility mode.
    #[must_use]
    pub fn new_with_options(
        conversations: Vec<Conversation>,
        current_cwd: String,
        include_subagents: bool,
    ) -> Self {
        let mut app = Self {
            conversations,
            visible: Vec::new(),
            selected: 0,
            query: String::new(),
            filter_mode: FilterMode::All,
            sort_mode: SortMode::Updated,
            focus: FocusTarget::Search,
            current_cwd,
            column_preset: ColumnPreset::Core,
            row_mode: RowMode::Compact,
            subagent_mode: if include_subagents {
                SubagentMode::Show
            } else {
                SubagentMode::Hide
            },
        };
        app.refresh_visible();
        app
    }

    fn refresh_visible(&mut self) {
        let query = self.query.to_lowercase();
        self.visible = self
            .conversations
            .iter()
            .enumerate()
            .filter(|(_, conversation)| {
                if !self.matches_filter(conversation) {
                    return false;
                }

                Self::matches_query(conversation, &query)
            })
            .map(|(index, _)| index)
            .collect();

        match self.sort_mode {
            SortMode::Updated => {
                self.visible.sort_by(|left, right| {
                    self.conversations[*right]
                        .updated_at_ms
                        .cmp(&self.conversations[*left].updated_at_ms)
                        .then_with(|| {
                            self.conversations[*left]
                                .display_title
                                .cmp(&self.conversations[*right].display_title)
                        })
                });
            }
            SortMode::Created => {
                self.visible.sort_by(|left, right| {
                    self.conversations[*right]
                        .created_at_ms
                        .cmp(&self.conversations[*left].created_at_ms)
                        .then_with(|| {
                            self.conversations[*left]
                                .display_title
                                .cmp(&self.conversations[*right].display_title)
                        })
                });
            }
        }

        if self.visible.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.visible.len() {
            self.selected = self.visible.len() - 1;
        }
    }

    fn matches_filter(&self, conversation: &Conversation) -> bool {
        let directory_matches = match self.filter_mode {
            FilterMode::All => true,
            FilterMode::CurrentDirectory => conversation.cwd == self.current_cwd,
        };
        let subagent_matches = match self.subagent_mode {
            SubagentMode::Hide => !conversation.is_subagent,
            SubagentMode::Show => true,
        };

        if !directory_matches {
            return false;
        }

        subagent_matches
    }

    fn toggle_subagent_mode(&mut self) {
        self.subagent_mode = self.subagent_mode.next();
        self.refresh_visible();
    }

    fn include_subagents(&self) -> bool {
        match self.subagent_mode {
            SubagentMode::Hide => false,
            SubagentMode::Show => true,
        }
    }

    fn is_showing_subagents(&self) -> bool {
        self.include_subagents()
    }

    fn matches_query(conversation: &Conversation, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }

        let haystack = normalize_search_text(&format!(
            "{} {} {} {}",
            conversation.display_title, conversation.preview, conversation.cwd, conversation.id
        ));
        let normalized_query = normalize_search_text(query);

        haystack.contains(&normalized_query)
    }

    #[cfg(test)]
    fn visible_ids(&self) -> Vec<String> {
        self.visible
            .iter()
            .map(|index| self.conversations[*index].id.clone())
            .collect()
    }

    fn selected_conversation(&self) -> Option<&Conversation> {
        pick_selected_conversation(&self.visible, self.selected, &self.conversations)
    }

    fn move_down(&mut self) {
        if self.visible.is_empty() {
            return;
        }

        self.selected = if self.selected + 1 < self.visible.len() {
            self.selected + 1
        } else {
            0
        };
    }

    fn move_up(&mut self) {
        if self.visible.is_empty() {
            return;
        }

        self.selected = if self.selected == 0 {
            self.visible.len() - 1
        } else {
            self.selected - 1
        };
    }

    fn append_query(&mut self, ch: char) {
        self.focus = FocusTarget::Search;
        self.query.push(ch);
        self.refresh_visible();
    }

    fn backspace_query(&mut self) {
        self.focus = FocusTarget::Search;
        self.query.pop();
        self.refresh_visible();
    }

    fn clear_query(&mut self) {
        self.focus = FocusTarget::Search;
        self.query.clear();
        self.refresh_visible();
    }

    fn escape(&mut self) -> bool {
        if self.query.is_empty() {
            true
        } else {
            self.clear_query();
            false
        }
    }

    fn cycle_focus_forward(&mut self) {
        self.focus = self.focus.next();
    }

    fn cycle_focus_backward(&mut self) {
        self.focus = self.focus.previous();
    }

    fn change_current_option_right(&mut self) {
        match self.focus {
            FocusTarget::Search => {}
            FocusTarget::Filter => {
                self.filter_mode = self.filter_mode.next();
                self.refresh_visible();
            }
            FocusTarget::Subagents => self.toggle_subagent_mode(),
            FocusTarget::Sort => {
                self.sort_mode = self.sort_mode.next();
                self.refresh_visible();
            }
        }
    }

    fn change_current_option_left(&mut self) {
        match self.focus {
            FocusTarget::Search => {}
            FocusTarget::Filter => {
                self.filter_mode = self.filter_mode.previous();
                self.refresh_visible();
            }
            FocusTarget::Subagents => {
                self.subagent_mode = self.subagent_mode.previous();
                self.refresh_visible();
            }
            FocusTarget::Sort => {
                self.sort_mode = self.sort_mode.previous();
                self.refresh_visible();
            }
        }
    }

    fn toggle_column_preset(&mut self) {
        self.column_preset = self.column_preset.toggle();
    }

    fn toggle_row_mode(&mut self) {
        self.row_mode = self.row_mode.toggle();
    }
}

/// Loads conversations from the local Codex `SQLite` state and session index.
///
/// # Errors
///
/// Returns an error when the local Codex database or session index cannot be
/// read or when a stored thread row is malformed.
pub fn load_conversations(db_path: &Path, session_index_path: &Path) -> Result<Vec<Conversation>> {
    load_conversations_with_options(db_path, session_index_path, false)
}

/// Loads conversations with optional inclusion of spawned subagent threads.
///
/// # Errors
///
/// Returns an error when the local Codex database or session index cannot be
/// read or when a stored thread row is malformed.
pub fn load_conversations_with_options(
    db_path: &Path,
    session_index_path: &Path,
    include_subagents: bool,
) -> Result<Vec<Conversation>> {
    let conversations = load_all_conversations(db_path, session_index_path)?;

    if include_subagents {
        Ok(conversations)
    } else {
        Ok(deduplicate_conversations(
            conversations
                .into_iter()
                .filter(|conversation| !conversation.is_subagent)
                .collect(),
        ))
    }
}

fn load_all_conversations(db_path: &Path, session_index_path: &Path) -> Result<Vec<Conversation>> {
    let connection = match Connection::open(db_path) {
        Ok(connection) => connection,
        Err(error) => {
            return Err(error).context(format!("failed to open {}", db_path.display()));
        }
    };
    let thread_names = load_thread_names(session_index_path)?;
    let query_support = load_thread_query_support(&connection)?;

    let query = format!(
        "select
            id,
            cwd,
            nullif(title, ''),
            nullif(preview, ''),
            nullif(first_user_message, ''),
            coalesce(recency_at_ms, 0),
            updated_at_ms,
            coalesce(nullif(created_at_ms, 0), created_at * 1000, updated_at_ms),
            {source_select},
            {thread_source_select}
        from threads
        where archived = 0
        order by coalesce(nullif(recency_at_ms, 0), updated_at_ms) desc",
        source_select = query_support.source_select,
        thread_source_select = query_support.thread_source_select
    );
    let mut statement = connection.prepare(&query)?;

    let mut rows = statement.raw_query();
    let mut conversations = Vec::new();

    while let Some(row) = next_thread_row(&mut rows)? {
        let is_subagent = row.is_subagent(&query_support.subagent_thread_ids);
        let thread_name = thread_names.get(&row.id).map(String::as_str);
        conversations.push(Conversation::try_from_row(row, thread_name, is_subagent)?);
    }

    Ok(deduplicate_conversations(conversations))
}

fn thread_row_from_sql_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
    let source = if row.as_ref().column_count() > 8 {
        row.get(8)?
    } else {
        None
    };
    let thread_source = if row.as_ref().column_count() > 9 {
        row.get(9)?
    } else {
        None
    };

    Ok(ThreadRow {
        id: row.get(0)?,
        cwd: row.get(1)?,
        title: row.get(2)?,
        preview: row.get(3)?,
        first_user_message: row.get(4)?,
        recency_at_ms: row.get(5)?,
        updated_at_ms: row.get(6)?,
        created_at_ms: row.get(7)?,
        source,
        thread_source,
    })
}

impl ThreadRow {
    fn is_subagent(&self, subagent_thread_ids: &HashSet<String>) -> bool {
        thread_row_is_subagent(self, subagent_thread_ids)
    }
}

fn load_thread_query_support(connection: &Connection) -> Result<ThreadQuerySupport> {
    let subagent_thread_ids = detect_subagent_thread_ids(connection)?;
    let has_source = column_exists(connection, "threads", "source")?;
    let has_thread_source = column_exists(connection, "threads", "thread_source")?;

    let source_select = if has_source {
        "nullif(source, '')"
    } else {
        "null"
    };
    let thread_source_select = if has_thread_source {
        "nullif(thread_source, '')"
    } else {
        "null"
    };

    Ok(ThreadQuerySupport {
        subagent_thread_ids,
        source_select,
        thread_source_select,
    })
}

fn detect_subagent_thread_ids(connection: &Connection) -> Result<HashSet<String>> {
    if !table_exists(connection, "thread_spawn_edges")? {
        return Ok(HashSet::new());
    }

    let child_thread_ids =
        connection.query_row(CHILD_THREAD_IDS_QUERY, [], |row| row.get::<_, String>(0))?;
    let child_thread_ids = serde_json::from_str::<Vec<String>>(&child_thread_ids)
        .context("failed to decode thread_spawn_edges.child_thread_id values")?;

    Ok(child_thread_ids.into_iter().collect())
}

fn table_exists(connection: &Connection, table_name: &str) -> Result<bool> {
    let exists = connection.query_row(
        "select exists(
            select 1
            from sqlite_master
            where type = 'table' and name = ?1
        )",
        [table_name],
        |row| row.get::<_, i64>(0),
    )?;

    Ok(exists != 0)
}

fn column_exists(connection: &Connection, table_name: &str, column_name: &str) -> Result<bool> {
    let exists = connection.query_row(
        "select exists(
            select 1
            from pragma_table_info(?1)
            where name = ?2
        )",
        [table_name, column_name],
        |row| row.get::<_, i64>(0),
    )?;

    Ok(exists != 0)
}

fn next_thread_row(rows: &mut rusqlite::Rows<'_>) -> Result<Option<ThreadRow>> {
    let next = rows.next();
    let next_row = next.context("failed to query thread rows")?;
    let Some(row) = next_row else {
        return Ok(None);
    };

    Ok(Some(
        thread_row_from_sql_row(row).context("failed to query thread rows")?,
    ))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

/// Returns the default Codex `SQLite` path under the current home directory.
#[must_use]
pub fn default_state_db_path() -> PathBuf {
    default_state_db_path_from_home(home_dir())
}

/// Returns the default Codex session index path under the current home directory.
#[must_use]
pub fn default_session_index_path() -> PathBuf {
    default_session_index_path_from_home(home_dir())
}

fn default_state_db_path_from_home(home_dir: Option<PathBuf>) -> PathBuf {
    match home_dir {
        Some(home_dir) => home_dir.join(".codex").join("state_5.sqlite"),
        None => PathBuf::from("~").join(".codex").join("state_5.sqlite"),
    }
}

fn default_session_index_path_from_home(home_dir: Option<PathBuf>) -> PathBuf {
    match home_dir {
        Some(home_dir) => home_dir.join(".codex").join("session_index.jsonl"),
        None => PathBuf::from("~")
            .join(".codex")
            .join("session_index.jsonl"),
    }
}

impl Conversation {
    /// Returns the stable Codex conversation identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    fn try_from_row(row: ThreadRow, thread_name: Option<&str>, is_subagent: bool) -> Result<Self> {
        let updated_at_ms = effective_updated_at_ms(row.recency_at_ms, row.updated_at_ms)
            .ok_or_else(|| missing_usable_timestamp_error(&row.id))?;

        Ok(Self {
            display_title: display_title(&row.id, thread_name, row.title.as_deref()),
            preview: preview_text(row.preview.as_deref(), row.first_user_message.as_deref()),
            id: row.id,
            rendered_cwd: render_cwd(&row.cwd),
            cwd: row.cwd,
            updated_at_ms,
            created_at_ms: row.created_at_ms.max(updated_at_ms),
            is_subagent,
        })
    }
}

fn missing_usable_timestamp_error(thread_id: &str) -> anyhow::Error {
    anyhow!("thread {thread_id} is missing a usable timestamp")
}

fn load_thread_names(session_index_path: &Path) -> Result<HashMap<String, String>> {
    let file = match File::open(session_index_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(HashMap::new());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to open {}", session_index_path.display()));
        }
    };

    let reader = BufReader::new(file);
    let mut thread_names = HashMap::new();

    for line in reader.lines() {
        let line = line.with_context(|| {
            format!("failed to read line from {}", session_index_path.display())
        })?;

        let entry: SessionIndexEntry = match serde_json::from_str(&line) {
            Ok(entry) => entry,
            Err(_) => {
                continue;
            }
        };

        let Some(thread_name) = entry.thread_name else {
            continue;
        };

        let trimmed = thread_name.trim();
        if !trimmed.is_empty() {
            thread_names.insert(entry.id, trimmed.to_string());
        }
    }

    Ok(thread_names)
}

fn render_cwd(cwd: &str) -> String {
    render_cwd_with_home(cwd, home_dir().as_deref())
}

fn effective_updated_at_ms(recency_at_ms: i64, updated_at_ms: i64) -> Option<i64> {
    if recency_at_ms > 0 {
        Some(recency_at_ms)
    } else if updated_at_ms > 0 {
        Some(updated_at_ms)
    } else {
        None
    }
}

/// Runs the default interactive picker flow.
///
/// # Errors
///
/// Returns an error when loading conversations, determining the current
/// directory, running the picker, or resuming the selected conversation fails.
pub fn run_default(config: RunConfig) -> Result<()> {
    run_with(
        config,
        |db_path, session_index_path| {
            load_conversations_with_options(db_path, session_index_path, true)
        },
        env::current_dir,
        select_conversation,
        resume_conversation,
    )
}

/// Runs the interactive picker flow with injected dependencies.
///
/// # Errors
///
/// Returns an error when loading conversations, determining the current
/// directory, selecting a conversation, or resuming the selected conversation
/// fails.
pub fn run_with<FLoad, FCwd, FSelect, FResume>(
    config: RunConfig,
    mut load_conversations: FLoad,
    mut current_dir: FCwd,
    mut select_conversation: FSelect,
    mut resume_conversation: FResume,
) -> Result<()>
where
    FLoad: FnMut(&Path, &Path) -> Result<Vec<Conversation>>,
    FCwd: FnMut() -> io::Result<PathBuf>,
    FSelect: FnMut(&mut App) -> Result<Conversation>,
    FResume: FnMut(&str, &str, &str, bool) -> Result<()>,
{
    run_with_impl(
        config,
        &mut load_conversations,
        &mut current_dir,
        &mut select_conversation,
        &mut resume_conversation,
    )
}

fn run_with_impl(
    config: RunConfig,
    load_conversations: &mut dyn FnMut(&Path, &Path) -> Result<Vec<Conversation>>,
    current_dir: &mut dyn FnMut() -> io::Result<PathBuf>,
    select_conversation: &mut dyn FnMut(&mut App) -> Result<Conversation>,
    resume_conversation: &mut dyn FnMut(&str, &str, &str, bool) -> Result<()>,
) -> Result<()> {
    let db_path = config.db_path.unwrap_or_else(default_state_db_path);
    let session_index_path = config
        .session_index_path
        .unwrap_or_else(default_session_index_path);
    let conversations = load_conversations(&db_path, &session_index_path)?;
    let current_cwd = current_dir()
        .context("failed to determine current directory")?
        .display()
        .to_string();

    if conversations.is_empty() {
        bail!("no Codex conversations found");
    }

    let mut app = App::new_with_options(conversations, current_cwd, config.include_subagents);
    let selected = select_conversation(&mut app)?;
    resume_conversation(
        &config.codex_bin,
        selected.id(),
        selected.cwd.as_str(),
        config.dry_run,
    )
}

/// Selects a conversation using the default terminal session implementation.
///
/// # Errors
///
/// Returns an error when terminal setup, drawing, event polling, event reading,
/// or restoration fails.
#[cfg(not(test))]
pub fn select_conversation(app: &mut App) -> Result<Conversation> {
    let test_terminal = debug_fake_terminal_enabled();
    if test_terminal {
        return app
            .selected_conversation()
            .cloned()
            .context("no conversations available to select");
    }

    select_conversation_with_session(
        app,
        init_terminal,
        |terminal, app| {
            run_picker_loop(
                app,
                |app| finish_draw(terminal.draw(|frame| render(frame, app))),
                || {
                    Ok(crossterm::event::poll(Duration::from_millis(
                        POLL_INTERVAL_MS,
                    ))?)
                },
                || Ok(crossterm::event::read()?),
            )
        },
        restore_terminal,
    )
}

#[cfg(test)]
/// Selects a conversation using the test terminal session implementation.
///
/// # Errors
///
/// Returns an error when drawing, event handling, or restoration fails.
pub fn select_conversation(app: &mut App) -> Result<Conversation> {
    let init = init_terminal;
    select_conversation_with_session(
        app,
        init,
        |terminal, app| {
            run_picker_loop(
                app,
                |app| finish_draw(terminal.draw(|frame| render(frame, app))),
                || Ok(true),
                || {
                    Ok(Event::Key(KeyEvent::new(
                        KeyCode::Enter,
                        KeyModifiers::NONE,
                    )))
                },
            )
        },
        restore_terminal,
    )
}

/// Selects a conversation using injected terminal lifecycle callbacks.
///
/// # Errors
///
/// Returns an error when initialization, selection, or restoration fails.
pub fn select_conversation_with_session<FInit, FRun, FRestore, T>(
    app: &mut App,
    mut init: FInit,
    mut run: FRun,
    mut restore: FRestore,
) -> Result<Conversation>
where
    FInit: FnMut() -> Result<T>,
    FRun: FnMut(&mut T, &mut App) -> Result<Conversation>,
    FRestore: FnMut(&mut T) -> Result<()>,
{
    with_terminal_session(&mut init, |terminal| run(terminal, app), &mut restore)
}

#[cfg(not(test))]
fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    if debug_fake_terminal_enabled() {
        return Terminal::new(CrosstermBackend::new(io::stdout()))
            .context("failed to create terminal");
    }

    init_terminal_session(
        || enable_raw_mode().context("failed to enable raw mode"),
        || {
            let mut stdout = io::stdout();
            execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
            Ok(stdout)
        },
        |stdout| Terminal::new(CrosstermBackend::new(stdout)).context("failed to create terminal"),
    )
}

#[cfg(test)]
fn init_terminal() -> Result<Terminal<TestBackend>> {
    Terminal::new(TestBackend::new(80, 24)).context("failed to create terminal")
}

#[cfg(not(test))]
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    if debug_fake_terminal_enabled() {
        return terminal.show_cursor().context("failed to restore cursor");
    }

    restore_terminal_session(
        terminal,
        || disable_raw_mode().context("failed to disable raw mode"),
        |terminal| {
            execute!(terminal.backend_mut(), LeaveAlternateScreen)
                .context("failed to leave alternate screen")
        },
        |terminal| terminal.show_cursor().context("failed to restore cursor"),
    )
}

#[cfg(test)]
fn restore_terminal(terminal: &mut Terminal<TestBackend>) -> Result<()> {
    terminal.show_cursor().context("failed to restore cursor")
}

/// Initializes a terminal session with injected raw-mode and backend builders.
///
/// # Errors
///
/// Returns an error when enabling raw mode, entering the alternate screen, or
/// constructing the terminal fails.
pub fn init_terminal_session<FEnable, FBackend, FTerminal, B, T>(
    mut enable_raw: FEnable,
    mut enter_alternate_screen: FBackend,
    mut build_terminal: FTerminal,
) -> Result<T>
where
    FEnable: FnMut() -> Result<()>,
    FBackend: FnMut() -> Result<B>,
    FTerminal: FnMut(B) -> Result<T>,
{
    init_terminal_session_impl(
        &mut enable_raw,
        &mut enter_alternate_screen,
        &mut build_terminal,
    )
}

fn init_terminal_session_impl<B, T>(
    enable_raw: &mut dyn FnMut() -> Result<()>,
    enter_alternate_screen: &mut dyn FnMut() -> Result<B>,
    build_terminal: &mut dyn FnMut(B) -> Result<T>,
) -> Result<T> {
    enable_raw()?;
    let backend = enter_alternate_screen()?;
    build_terminal(backend)
}

/// Restores a terminal session with injected cleanup callbacks.
///
/// # Errors
///
/// Returns an error when disabling raw mode, leaving the alternate screen, or
/// restoring the cursor fails.
/// Runs work inside an initialized terminal session and always attempts restore.
///
/// # Errors
///
/// Returns initialization, restore, or work errors from the provided callbacks.
/// Resumes the selected conversation with the configured `codex` binary.
///
/// # Errors
///
/// Returns an error when writing dry-run output, spawning the subprocess, or
/// the subprocess exits unsuccessfully.
pub fn resume_conversation(
    codex_bin: &str,
    conversation_id: &str,
    cwd: &str,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        let mut stdout = io::stdout();
        return write_dry_run_output(&mut stdout, codex_bin, conversation_id, cwd);
    }

    resume_subprocess_conversation(codex_bin, conversation_id, cwd)
}

fn resume_command_status(
    codex_bin: &str,
    conversation_id: &str,
    cwd: &str,
) -> Result<std::process::ExitStatus> {
    match Command::new(codex_bin)
        .args(["resume", "-C", cwd, conversation_id])
        .current_dir(cwd)
        .status()
    {
        Ok(status) => Ok(status),
        Err(error) => Err(error).context(format!(
            "failed to execute `{codex_bin} resume -C {cwd} {conversation_id}`"
        )),
    }
}

enum ResumeStatusResult {
    Status(std::process::ExitStatus),
    Error(anyhow::Error),
}

fn classify_resume_status(result: Result<std::process::ExitStatus>) -> ResumeStatusResult {
    match result {
        Ok(status) => ResumeStatusResult::Status(status),
        Err(error) => ResumeStatusResult::Error(error),
    }
}

fn resume_subprocess_conversation(codex_bin: &str, conversation_id: &str, cwd: &str) -> Result<()> {
    let status =
        match classify_resume_status(resume_command_status(codex_bin, conversation_id, cwd)) {
            ResumeStatusResult::Status(status) => status,
            ResumeStatusResult::Error(error) => return Err(error),
        };

    if status.success() {
        Ok(())
    } else {
        bail!("`{codex_bin} resume -C {cwd} {conversation_id}` exited with {status}");
    }
}

#[cfg(not(test))]
fn debug_fake_terminal_enabled() -> bool {
    cfg!(debug_assertions) && env::var_os("CDX_TEST_FAKE_TERMINAL").is_some()
}

/// Runs the interactive picker loop using injected draw and event callbacks.
///
/// # Errors
///
/// Returns an error when drawing fails, event polling fails, event reading
/// fails, or the picker is cancelled through a key handler error path.
pub fn run_picker_loop<FDraw, FPoll, FRead>(
    app: &mut App,
    mut draw: FDraw,
    mut poll: FPoll,
    mut read: FRead,
) -> Result<Conversation>
where
    FDraw: FnMut(&App) -> Result<()>,
    FPoll: FnMut() -> Result<bool>,
    FRead: FnMut() -> Result<Event>,
{
    run_picker_loop_impl(app, &mut draw, &mut poll, &mut read)
}

/// Renders the picker UI into the provided frame.
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let layout = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .split(frame.area());

    render_header(frame, layout[0]);
    render_controls(frame, layout[1], app);
    render_table(frame, layout[3], app);
    render_status(frame, layout[4], app);
    render_footer(frame, layout[5]);
}

fn render_header(frame: &mut Frame<'_>, area: Rect) {
    let title = Paragraph::new("Resume a previous session").style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(title, area);
}

fn render_controls(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let search_style = if app.focus == FocusTarget::Search {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let search_value = if app.query.is_empty() {
        "Type to search".to_string()
    } else {
        app.query.clone()
    };

    let controls_variant = controls_variant_for_width(app, area.width);
    let controls_width = controls_variant.width(app);
    let layout = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(controls_width.min(area.width)),
    ])
    .split(area);

    let search = Paragraph::new(search_value).style(search_style);
    frame.render_widget(search, layout[0]);

    let filter_style = if app.focus == FocusTarget::Filter {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let sort_style = if app.focus == FocusTarget::Sort {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let subagent_style = if app.focus == FocusTarget::Subagents {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let controls =
        Paragraph::new(controls_variant.line(app, filter_style, subagent_style, sort_style))
            .alignment(Alignment::Right);
    frame.render_widget(controls, layout[1]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlsVariant {
    Full,
    Compact,
    Tight,
    Minimal,
}

impl ControlsVariant {
    fn width(self, app: &App) -> u16 {
        match self {
            Self::Full => controls_width(&[
                "Filter: ",
                app.filter_mode.label(),
                "   ",
                "Threads: ",
                app.subagent_mode.label(),
                "   ",
                "Sort: ",
                app.sort_mode.label(),
            ]),
            Self::Compact => controls_width(&[
                "F: ",
                compact_filter_label(app.filter_mode),
                "   ",
                "T: ",
                compact_subagent_label(app.subagent_mode),
                "   ",
                "S: ",
                compact_sort_label(app.sort_mode),
            ]),
            Self::Tight => controls_width(&[
                "F:",
                tight_filter_label(app.filter_mode),
                " ",
                "T:",
                tight_subagent_label(app.subagent_mode),
                " ",
                "S:",
                tight_sort_label(app.sort_mode),
            ]),
            Self::Minimal => controls_width(&[
                "F",
                tight_filter_label(app.filter_mode),
                " ",
                "T",
                tight_subagent_label(app.subagent_mode),
                " ",
                "S",
                tight_sort_label(app.sort_mode),
            ]),
        }
    }

    fn line(
        self,
        app: &App,
        filter_style: Style,
        subagent_style: Style,
        sort_style: Style,
    ) -> Line<'static> {
        let muted = Style::default().fg(Color::DarkGray);

        match self {
            Self::Full => Line::from(vec![
                Span::styled("Filter: ", muted),
                Span::styled(app.filter_mode.label(), filter_style),
                Span::raw("   "),
                Span::styled("Threads: ", muted),
                Span::styled(app.subagent_mode.label(), subagent_style),
                Span::raw("   "),
                Span::styled("Sort: ", muted),
                Span::styled(app.sort_mode.label(), sort_style),
            ]),
            Self::Compact => Line::from(vec![
                Span::styled("F: ", muted),
                Span::styled(compact_filter_label(app.filter_mode), filter_style),
                Span::raw("   "),
                Span::styled("T: ", muted),
                Span::styled(compact_subagent_label(app.subagent_mode), subagent_style),
                Span::raw("   "),
                Span::styled("S: ", muted),
                Span::styled(compact_sort_label(app.sort_mode), sort_style),
            ]),
            Self::Tight => Line::from(vec![
                Span::styled("F:", muted),
                Span::styled(tight_filter_label(app.filter_mode), filter_style),
                Span::raw(" "),
                Span::styled("T:", muted),
                Span::styled(tight_subagent_label(app.subagent_mode), subagent_style),
                Span::raw(" "),
                Span::styled("S:", muted),
                Span::styled(tight_sort_label(app.sort_mode), sort_style),
            ]),
            Self::Minimal => Line::from(vec![
                Span::styled("F", muted),
                Span::styled(tight_filter_label(app.filter_mode), filter_style),
                Span::raw(" "),
                Span::styled("T", muted),
                Span::styled(tight_subagent_label(app.subagent_mode), subagent_style),
                Span::raw(" "),
                Span::styled("S", muted),
                Span::styled(tight_sort_label(app.sort_mode), sort_style),
            ]),
        }
    }
}

fn controls_width(parts: &[&str]) -> u16 {
    let mut total = 0_usize;
    let mut index = 0;

    while index < parts.len() {
        total = total.saturating_add(parts[index].len());
        index += 1;
    }

    u16::try_from(total).unwrap_or(u16::MAX)
}

fn controls_variant_for_width(app: &App, area_width: u16) -> ControlsVariant {
    let search_width = if app.query.is_empty() {
        MIN_SEARCH_WIDTH
    } else {
        MIN_SEARCH_WIDTH.min(area_width)
    };
    let controls_room = area_width
        .saturating_sub(search_width)
        .saturating_sub(CONTROLS_SAFE_MARGIN);

    if controls_room >= ControlsVariant::Full.width(app) {
        ControlsVariant::Full
    } else if controls_room >= ControlsVariant::Compact.width(app) {
        ControlsVariant::Compact
    } else if controls_room >= ControlsVariant::Tight.width(app) {
        ControlsVariant::Tight
    } else {
        ControlsVariant::Minimal
    }
}

const fn compact_filter_label(mode: FilterMode) -> &'static str {
    match mode {
        FilterMode::CurrentDirectory => "[Cwd] All",
        FilterMode::All => "Cwd [All]",
    }
}

const fn compact_subagent_label(mode: SubagentMode) -> &'static str {
    match mode {
        SubagentMode::Hide => "[Primary] All",
        SubagentMode::Show => "Primary [All]",
    }
}

const fn compact_sort_label(mode: SortMode) -> &'static str {
    match mode {
        SortMode::Updated => "[Updated] Created",
        SortMode::Created => "Updated [Created]",
    }
}

const fn tight_filter_label(mode: FilterMode) -> &'static str {
    match mode {
        FilterMode::CurrentDirectory => "[C]",
        FilterMode::All => "[A]",
    }
}

const fn tight_subagent_label(mode: SubagentMode) -> &'static str {
    match mode {
        SubagentMode::Hide => "[P]",
        SubagentMode::Show => "[A]",
    }
}

const fn tight_sort_label(mode: SortMode) -> &'static str {
    match mode {
        SortMode::Updated => "[U]",
        SortMode::Created => "[C]",
    }
}

fn render_table(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.visible.is_empty() {
        let empty = Paragraph::new("No conversations match the current search/filter.")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Clear, area);
        frame.render_widget(empty, area);
        return;
    }

    let excerpt_width = excerpt_column_width(area, app.column_preset);
    let mut rows = Vec::with_capacity(app.visible.len());
    let mut visible_position = 0;
    while visible_position < app.visible.len() {
        let conversation_index = app.visible[visible_position];
        rows.push(table_row(
            &app.conversations[conversation_index],
            app.column_preset,
            app.row_mode,
            excerpt_width,
        ));
        visible_position += 1;
    }

    let (header, widths) = table_columns(app.column_preset);

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(TABLE_COLUMN_SPACING)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");

    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);

    let mut scrollbar_state = ScrollbarState::new(app.visible.len())
        .position(app.selected.min(app.visible.len().saturating_sub(1)));
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
        area,
        &mut scrollbar_state,
    );
}

fn table_columns(column_preset: ColumnPreset) -> (Row<'static>, Vec<Constraint>) {
    match column_preset {
        ColumnPreset::Core => (
            Row::new(vec![
                Cell::from("Directory"),
                Cell::from("Conversation"),
                Cell::from("Excerpt"),
            ])
            .style(Style::default().fg(Color::DarkGray)),
            vec![
                Constraint::Length(28),
                Constraint::Length(26),
                Constraint::Min(36),
            ],
        ),
        ColumnPreset::Full => (
            Row::new(vec![
                Cell::from("Age"),
                Cell::from("Updated"),
                Cell::from("Directory"),
                Cell::from("Conversation"),
                Cell::from("Excerpt"),
            ])
            .style(Style::default().fg(Color::DarkGray)),
            vec![
                Constraint::Length(9),
                Constraint::Length(16),
                Constraint::Length(24),
                Constraint::Length(22),
                Constraint::Min(28),
            ],
        ),
    }
}

fn table_row(
    conversation: &Conversation,
    column_preset: ColumnPreset,
    row_mode: RowMode,
    excerpt_width: usize,
) -> Row<'static> {
    let age = format_relative_time(conversation.updated_at_ms);
    let updated = match format_actual_time(conversation.updated_at_ms) {
        Some(updated) => updated,
        None => "unknown".to_string(),
    };
    let dir = truncate_chars(&conversation.rendered_cwd, 26).into_owned();
    let bucket = recency_bucket(conversation.updated_at_ms);
    let mut excerpt = conversation.preview.clone();
    if excerpt.is_empty() {
        excerpt = "[no preview]".to_string();
    }
    let excerpt_lines = excerpt_lines(&excerpt, excerpt_width, row_mode);

    let title_cell = match row_mode {
        RowMode::Compact => Cell::from(Span::styled(
            truncate_chars(&conversation.display_title, 24).into_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        RowMode::Comfortable => Cell::from(vec![
            Line::from(Span::styled(
                truncate_chars(&conversation.display_title, 24).into_owned(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                truncate_chars(&dir, 24).into_owned(),
                Style::default().fg(Color::Gray),
            )),
        ]),
    };

    let excerpt_cell = match row_mode {
        RowMode::Compact => Cell::from(excerpt_lines[0].clone()),
        RowMode::Comfortable => Cell::from(vec![
            Line::from(excerpt_lines[0].clone()),
            Line::from(excerpt_lines[1].clone()),
        ]),
    };

    let row = match column_preset {
        ColumnPreset::Core => Row::new(vec![
            Cell::from(dir).style(Style::default().fg(Color::Gray)),
            title_cell,
            excerpt_cell,
        ]),
        ColumnPreset::Full => Row::new(vec![
            Cell::from(age).style(bucket.style()),
            Cell::from(updated).style(bucket.style()),
            Cell::from(dir).style(Style::default().fg(Color::Gray)),
            title_cell,
            excerpt_cell,
        ]),
    };

    match row_mode {
        RowMode::Compact => row,
        RowMode::Comfortable => row.height(2),
    }
}

fn excerpt_lines(value: &str, width: usize, row_mode: RowMode) -> Vec<String> {
    match row_mode {
        RowMode::Compact => vec![truncate_to_width(value, width).into_owned()],
        RowMode::Comfortable => {
            if width == 0 {
                return vec![String::new(), String::new()];
            }

            let chars = value.chars().collect::<Vec<_>>();
            let first_line = chars.iter().take(width).collect::<String>();
            let remaining = chars.iter().skip(width).collect::<String>();
            let mut second_line = remaining.chars().take(width).collect::<String>();

            if chars.len() > width.saturating_mul(2) {
                second_line = truncate_to_width(&remaining, width).into_owned();
            }

            vec![first_line, second_line]
        }
    }
}

fn excerpt_column_width(area: Rect, column_preset: ColumnPreset) -> usize {
    let (_, widths) = table_columns(column_preset);
    let spacing_width = TABLE_COLUMN_SPACING
        .saturating_mul(u16::try_from(widths.len().saturating_sub(1)).unwrap_or(u16::MAX));
    let available_width = area
        .width
        .saturating_sub(TABLE_HIGHLIGHT_WIDTH)
        .saturating_sub(TABLE_SCROLLBAR_WIDTH)
        .saturating_sub(spacing_width);
    let columns = Layout::horizontal(widths).split(Rect::new(0, 0, available_width, 1));

    usize::from(columns[columns.len().saturating_sub(1)].width)
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let total = app.visible.len();
    let position = app.selected.saturating_add(1).min(total);
    let selected_cwd = match app.selected_conversation() {
        Some(conversation) => conversation.rendered_cwd.as_str(),
        None => "",
    };

    let status = Paragraph::new(Line::from(vec![
        Span::styled(selected_cwd.to_string(), Style::default().fg(Color::Gray)),
        Span::raw(format!("  {position}/{total}")),
        Span::raw("  "),
        Span::styled(
            format!("Cols: {}", app.column_preset.label()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("View: {}", app.row_mode.label()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "Threads: {}",
                if app.is_showing_subagents() {
                    "All"
                } else {
                    "Primary"
                }
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(status, area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect) {
    let footer = Paragraph::new(vec![
        Line::from("enter resume   esc exit   ctrl+c quit   tab focus search/filter/threads/sort   ←/→ or space change option"),
        Line::from("type search   backspace delete   ctrl+u clear   ctrl+v columns   ctrl+o view   ↑/↓ browse"),
    ])
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn format_actual_time(timestamp_ms: i64) -> Option<String> {
    let timestamp = Local.timestamp_millis_opt(timestamp_ms).single();
    timestamp.map(format_local_time)
}

fn format_local_time(value: DateTime<Local>) -> String {
    value.format("%Y-%m-%d %H:%M").to_string()
}

fn format_relative_time(timestamp_ms: i64) -> String {
    format_relative_time_at(Utc::now(), timestamp_ms)
}

fn format_relative_delta(delta: TimeDelta) -> String {
    if delta < TimeDelta::minutes(1) {
        "just now".to_string()
    } else if delta < TimeDelta::hours(1) {
        format!("{}m ago", delta.num_minutes())
    } else if delta < TimeDelta::days(1) {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

fn format_relative_time_at(now: DateTime<Utc>, timestamp_ms: i64) -> String {
    let timestamp = DateTime::<Utc>::from_timestamp_millis(timestamp_ms);

    match timestamp {
        Some(value) => format_relative_delta(now.signed_duration_since(value)),
        None => "unknown".to_string(),
    }
}

fn recency_bucket(timestamp_ms: i64) -> RecencyBucket {
    recency_bucket_at(Utc::now(), timestamp_ms)
}

fn recency_bucket_from_delta(delta: TimeDelta) -> RecencyBucket {
    if delta <= TimeDelta::hours(72) {
        RecencyBucket::Recent
    } else if delta <= TimeDelta::days(7) {
        RecencyBucket::ThisWeek
    } else {
        RecencyBucket::Stale
    }
}

fn recency_bucket_at(now: DateTime<Utc>, timestamp_ms: i64) -> RecencyBucket {
    let timestamp = DateTime::<Utc>::from_timestamp_millis(timestamp_ms);

    match timestamp {
        Some(value) => recency_bucket_from_delta(now.signed_duration_since(value)),
        None => RecencyBucket::Stale,
    }
}

#[cfg(test)]
mod tests;
