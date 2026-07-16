//! Terminal UI for browsing and resuming local Codex conversations.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local, TimeDelta, TimeZone, Utc};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState,
};
use rusqlite::Connection;
use serde::Deserialize;

const PREVIEW_LIMIT: usize = 120;
const POLL_INTERVAL_MS: u64 = 250;

#[derive(Debug, Parser)]
#[command(author, version, about = "Global Codex conversation picker")]
struct Cli {
    #[arg(long)]
    db_path: Option<PathBuf>,
    #[arg(long)]
    session_index_path: Option<PathBuf>,
    #[arg(long, default_value = "codex")]
    codex_bin: String,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Conversation {
    id: String,
    cwd: String,
    rendered_cwd: String,
    display_title: String,
    preview: String,
    updated_at_ms: i64,
    created_at_ms: i64,
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
}

#[derive(Debug, Deserialize)]
struct SessionIndexEntry {
    id: String,
    thread_name: Option<String>,
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
    Sort,
}

impl FocusTarget {
    fn next(self) -> Self {
        match self {
            Self::Search => Self::Filter,
            Self::Filter => Self::Sort,
            Self::Sort => Self::Search,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Search => Self::Sort,
            Self::Filter => Self::Search,
            Self::Sort => Self::Filter,
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

#[derive(Debug)]
struct App {
    conversations: Vec<Conversation>,
    visible: Vec<usize>,
    selected: usize,
    query: String,
    filter_mode: FilterMode,
    sort_mode: SortMode,
    focus: FocusTarget,
    current_cwd: String,
}

impl App {
    fn new(conversations: Vec<Conversation>, current_cwd: String) -> Self {
        let mut app = Self {
            conversations,
            visible: Vec::new(),
            selected: 0,
            query: String::new(),
            filter_mode: FilterMode::All,
            sort_mode: SortMode::Updated,
            focus: FocusTarget::Search,
            current_cwd,
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
                self.matches_filter(conversation) && Self::matches_query(conversation, &query)
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
        match self.filter_mode {
            FilterMode::All => true,
            FilterMode::CurrentDirectory => conversation.cwd == self.current_cwd,
        }
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
        self.visible
            .get(self.selected)
            .and_then(|index| self.conversations.get(*index))
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
            FocusTarget::Sort => {
                self.sort_mode = self.sort_mode.previous();
                self.refresh_visible();
            }
        }
    }
}

fn main() {
    if let Err(error) = run() {
        let _stderr_write_result = writeln!(io::stderr(), "cdx: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db_path.unwrap_or_else(default_state_db_path);
    let session_index_path = cli
        .session_index_path
        .unwrap_or_else(default_session_index_path);
    let conversations = load_conversations(&db_path, &session_index_path)?;

    if conversations.is_empty() {
        bail!("no Codex conversations found in {}", db_path.display());
    }

    let current_cwd = env::current_dir()
        .context("failed to determine current directory")?
        .display()
        .to_string();
    let mut app = App::new(conversations, current_cwd);
    let selected = select_conversation(&mut app)?;
    resume_conversation(&cli.codex_bin, &selected.id, cli.dry_run)
}

fn load_conversations(db_path: &Path, session_index_path: &Path) -> Result<Vec<Conversation>> {
    let connection = Connection::open(db_path)
        .with_context(|| format!("failed to open {}", db_path.display()))?;
    let thread_names = load_thread_names(session_index_path)?;

    let mut statement = connection.prepare(
        "select
            id,
            cwd,
            nullif(title, ''),
            nullif(preview, ''),
            nullif(first_user_message, ''),
            coalesce(recency_at_ms, 0),
            updated_at_ms,
            coalesce(nullif(created_at_ms, 0), created_at * 1000, updated_at_ms)
        from threads
        where archived = 0
        order by coalesce(nullif(recency_at_ms, 0), updated_at_ms) desc",
    )?;

    let rows = statement.query_map([], |row| {
        Ok(ThreadRow {
            id: row.get(0)?,
            cwd: row.get(1)?,
            title: row.get(2)?,
            preview: row.get(3)?,
            first_user_message: row.get(4)?,
            recency_at_ms: row.get(5)?,
            updated_at_ms: row.get(6)?,
            created_at_ms: row.get(7)?,
        })
    })?;

    let conversations = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to query thread rows")?
        .into_iter()
        .map(|row| {
            let thread_name = thread_names.get(&row.id).map(String::as_str);
            Conversation::try_from_row(row, thread_name)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(deduplicate_conversations(conversations))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn default_state_db_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".codex")
        .join("state_5.sqlite")
}

fn default_session_index_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".codex")
        .join("session_index.jsonl")
}

impl Conversation {
    fn try_from_row(row: ThreadRow, thread_name: Option<&str>) -> Result<Self> {
        let updated_at_ms = effective_updated_at_ms(row.recency_at_ms, row.updated_at_ms)
            .ok_or_else(|| anyhow!("thread {} is missing a usable timestamp", row.id))?;

        Ok(Self {
            display_title: display_title(&row.id, thread_name, row.title.as_deref()),
            preview: preview_text(row.preview.as_deref(), row.first_user_message.as_deref()),
            id: row.id,
            rendered_cwd: render_cwd(&row.cwd),
            cwd: row.cwd,
            updated_at_ms,
            created_at_ms: row.created_at_ms.max(updated_at_ms),
        })
    }
}

fn load_thread_names(session_index_path: &Path) -> Result<HashMap<String, String>> {
    let file = match File::open(session_index_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
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
            Err(_) => continue,
        };

        if let Some(thread_name) = entry.thread_name {
            let trimmed = thread_name.trim();
            if !trimmed.is_empty() {
                thread_names.insert(entry.id, trimmed.to_string());
            }
        }
    }

    Ok(thread_names)
}

fn display_title<'a>(id: &'a str, thread_name: Option<&'a str>, title: Option<&'a str>) -> String {
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

fn preview_text(preview: Option<&str>, first_user_message: Option<&str>) -> String {
    let source = preview
        .or(first_user_message)
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    truncate_chars(&source, PREVIEW_LIMIT).into_owned()
}

fn render_cwd(cwd: &str) -> String {
    let Some(home_dir) = home_dir() else {
        return cwd.to_string();
    };

    let developer_root = home_dir.join("Developer");
    let cwd_path = Path::new(cwd);

    cwd_path.strip_prefix(&developer_root).ok().map_or_else(
        || cwd.to_string(),
        |suffix| {
            let suffix = suffix.display().to_string();
            if suffix.is_empty() {
                "~/Developer".to_string()
            } else {
                format!("~/Developer/{suffix}")
            }
        },
    )
}

fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn deduplicate_conversations(conversations: Vec<Conversation>) -> Vec<Conversation> {
    let mut seen_ids = HashSet::new();

    conversations
        .into_iter()
        .filter(|conversation| seen_ids.insert(conversation.id.clone()))
        .collect()
}

fn truncate_chars(value: &str, limit: usize) -> Cow<'_, str> {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(limit).collect();

    if chars.next().is_some() {
        Cow::Owned(format!("{truncated}…"))
    } else {
        Cow::Borrowed(value)
    }
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

fn select_conversation(app: &mut App) -> Result<Conversation> {
    let mut terminal = init_terminal()?;
    let result = run_picker(&mut terminal, app);
    restore_terminal(&mut terminal)?;
    result
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed to create terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")
}

fn run_picker(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<Conversation> {
    loop {
        terminal.draw(|frame| render(frame, app))?;

        if !event::poll(Duration::from_millis(POLL_INTERVAL_MS))? {
            continue;
        }

        if let Event::Key(key) = event::read()?
            && let Some(conversation) = handle_key_event(app, key)?
        {
            return Ok(conversation);
        }
    }
}

fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<Option<Conversation>> {
    if key.kind != KeyEventKind::Press {
        return Ok(None);
    }

    match key.code {
        KeyCode::Up => app.move_up(),
        KeyCode::Down => app.move_down(),
        KeyCode::Left => app.change_current_option_left(),
        KeyCode::Right => app.change_current_option_right(),
        KeyCode::Tab => app.cycle_focus_forward(),
        KeyCode::BackTab => app.cycle_focus_backward(),
        KeyCode::Backspace => app.backspace_query(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.clear_query();
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

fn render(frame: &mut Frame<'_>, app: &App) {
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
    let layout = Layout::horizontal([Constraint::Fill(1), Constraint::Length(38)])
        .flex(Flex::SpaceBetween)
        .split(area);

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

    let controls = Paragraph::new(Line::from(vec![
        Span::styled("Filter: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.filter_mode.label(), filter_style),
        Span::raw("   "),
        Span::styled("Sort: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.sort_mode.label(), sort_style),
    ]));
    frame.render_widget(controls, layout[1]);
}

fn render_table(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.visible.is_empty() {
        let empty = Paragraph::new("No conversations match the current search/filter.")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Clear, area);
        frame.render_widget(empty, area);
        return;
    }

    let rows = app
        .visible
        .iter()
        .map(|index| &app.conversations[*index])
        .map(table_row)
        .collect::<Vec<_>>();

    let header = Row::new(vec![
        Cell::from("Age"),
        Cell::from("Updated"),
        Cell::from("Directory"),
        Cell::from("Conversation"),
        Cell::from("Excerpt"),
    ])
    .style(Style::default().fg(Color::DarkGray));

    let widths = [
        Constraint::Length(9),
        Constraint::Length(16),
        Constraint::Length(28),
        Constraint::Length(24),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
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

fn table_row(conversation: &Conversation) -> Row<'static> {
    let age = format_relative_time(conversation.updated_at_ms);
    let updated =
        format_actual_time(conversation.updated_at_ms).unwrap_or_else(|| "unknown".to_string());
    let dir = truncate_chars(&conversation.rendered_cwd, 26).into_owned();
    let bucket = recency_bucket(conversation.updated_at_ms);

    Row::new(vec![
        Cell::from(age).style(bucket.style()),
        Cell::from(updated).style(bucket.style()),
        Cell::from(dir).style(Style::default().fg(Color::Gray)),
        Cell::from(Span::styled(
            conversation.display_title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(if conversation.preview.is_empty() {
            "[no preview]".to_string()
        } else {
            conversation.preview.clone()
        }),
    ])
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let total = app.visible.len();
    let position = if total == 0 { 0 } else { app.selected + 1 };
    let selected_cwd = app
        .selected_conversation()
        .map_or("", |conversation| conversation.rendered_cwd.as_str());

    let status = Paragraph::new(Line::from(vec![
        Span::styled(selected_cwd.to_string(), Style::default().fg(Color::Gray)),
        Span::raw(format!("  {position}/{total}")),
    ]));
    frame.render_widget(status, area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect) {
    let footer = Paragraph::new(vec![
        Line::from("enter resume   esc exit   ctrl+c quit   tab focus search/filter/sort   ←/→ change option"),
        Line::from("type search   backspace delete   ctrl+u clear   ↑/↓ browse"),
    ])
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn format_actual_time(timestamp_ms: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
}

fn format_relative_time(timestamp_ms: i64) -> String {
    format_relative_time_at(Utc::now(), timestamp_ms)
}

fn format_relative_time_at(now: DateTime<Utc>, timestamp_ms: i64) -> String {
    let Some(timestamp) = DateTime::<Utc>::from_timestamp_millis(timestamp_ms) else {
        return "unknown".to_string();
    };
    let delta = now.signed_duration_since(timestamp);

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

fn recency_bucket(timestamp_ms: i64) -> RecencyBucket {
    recency_bucket_at(Utc::now(), timestamp_ms)
}

fn recency_bucket_at(now: DateTime<Utc>, timestamp_ms: i64) -> RecencyBucket {
    let Some(timestamp) = DateTime::<Utc>::from_timestamp_millis(timestamp_ms) else {
        return RecencyBucket::Stale;
    };
    let delta = now.signed_duration_since(timestamp);

    if delta <= TimeDelta::hours(72) {
        RecencyBucket::Recent
    } else if delta <= TimeDelta::days(7) {
        RecencyBucket::ThisWeek
    } else {
        RecencyBucket::Stale
    }
}

fn resume_conversation(codex_bin: &str, conversation_id: &str, dry_run: bool) -> Result<()> {
    if dry_run {
        writeln!(io::stdout(), "{codex_bin} resume {conversation_id}")
            .context("failed to write dry-run output")?;
        return Ok(());
    }

    let status = Command::new(codex_bin)
        .args(["resume", conversation_id])
        .status()
        .with_context(|| format!("failed to execute `{codex_bin} resume {conversation_id}`"))?;

    if status.success() {
        Ok(())
    } else {
        bail!("`{codex_bin} resume {conversation_id}` exited with {status}");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rusqlite::Connection;

    use super::{
        App, Conversation, FilterMode, FocusTarget, PREVIEW_LIMIT, RecencyBucket, SortMode,
        ThreadRow, deduplicate_conversations, default_session_index_path, default_state_db_path,
        display_title, effective_updated_at_ms, format_actual_time, format_relative_time,
        format_relative_time_at, handle_key_event, home_dir, load_conversations, load_thread_names,
        normalize_search_text, preview_text, recency_bucket_at, render, render_cwd,
        resume_conversation,
    };

    #[test]
    fn falls_back_to_id_when_title_missing() {
        assert_eq!(
            display_title("0199-thread", None, Some("   ")),
            "0199-thread".to_string()
        );
    }

    #[test]
    fn prefers_thread_name_when_available() {
        assert_eq!(
            display_title("0199-thread", Some("woodpecker"), Some("long prompt title")),
            "woodpecker".to_string()
        );
    }

    #[test]
    fn prefers_id_over_long_prompt_title_when_thread_name_missing() {
        assert_eq!(
            display_title("0199-thread", None, Some("very long prompt title")),
            "0199-thread".to_string()
        );
    }

    #[test]
    fn falls_back_to_title_when_id_is_blank() {
        assert_eq!(
            display_title("   ", None, Some("named conversation")),
            "named conversation".to_string()
        );
    }

    #[test]
    fn normalizes_preview_from_first_user_message() {
        let preview = preview_text(None, Some("first line\nsecond line\tthird"));

        assert_eq!(preview, "first line second line third".to_string());
    }

    #[test]
    fn prefers_preview_text_when_available() {
        let preview = preview_text(Some("short preview"), Some("ignored fallback"));

        assert_eq!(preview, "short preview".to_string());
    }

    #[test]
    fn truncates_preview_to_requested_limit() {
        let preview = preview_text(None, Some(&"a".repeat(PREVIEW_LIMIT + 10)));

        assert_eq!(preview.chars().count(), PREVIEW_LIMIT + 1);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn prefers_recency_timestamp_when_available() {
        assert_eq!(effective_updated_at_ms(5_000, 1_000), Some(5_000));
    }

    #[test]
    fn falls_back_to_updated_timestamp_when_recency_missing() {
        assert_eq!(effective_updated_at_ms(0, 1_000), Some(1_000));
    }

    #[test]
    fn conversation_uses_preview_or_first_user_message() {
        let conversation_result = Conversation::try_from_row(
            ThreadRow {
                id: "thread-1".to_string(),
                cwd: "/tmp/example".to_string(),
                title: None,
                preview: None,
                first_user_message: Some("hello\nworld".to_string()),
                recency_at_ms: 10,
                updated_at_ms: 5,
                created_at_ms: 8,
            },
            Some("thread-name"),
        );
        let Ok(conversation) = conversation_result else {
            unreachable!("thread should normalize");
        };

        assert_eq!(conversation.display_title, "thread-name");
        assert_eq!(conversation.preview, "hello world");
        assert_eq!(conversation.updated_at_ms, 10);
        assert_eq!(conversation.created_at_ms, 10);
        assert_eq!(conversation.rendered_cwd, "/tmp/example");
    }

    #[test]
    fn relative_time_uses_days_for_old_entries() {
        let old_ms = (chrono::Utc::now() - chrono::TimeDelta::days(3)).timestamp_millis();

        assert_eq!(format_relative_time(old_ms), "3d ago");
    }

    #[test]
    fn relative_time_formats_minutes_and_hours() {
        let now = chrono::Utc::now();
        let minute_ms = (now - chrono::TimeDelta::minutes(7)).timestamp_millis();
        let hour_ms = (now - chrono::TimeDelta::hours(9)).timestamp_millis();

        assert_eq!(format_relative_time_at(now, minute_ms), "7m ago");
        assert_eq!(format_relative_time_at(now, hour_ms), "9h ago");
    }

    #[test]
    fn format_actual_time_returns_none_for_invalid_timestamp() {
        assert_eq!(format_actual_time(i64::MAX), None);
    }

    #[test]
    fn relative_time_handles_just_now_and_unknown_values() {
        let now = chrono::Utc::now();

        assert_eq!(
            format_relative_time_at(now, now.timestamp_millis()),
            "just now".to_string()
        );
        assert_eq!(
            format_relative_time_at(now, i64::MAX),
            "unknown".to_string()
        );
    }

    #[test]
    fn app_filters_by_query_across_title_preview_and_directory() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "Checkout bug", "fails on save", 30, 10),
                conversation("2", "/tmp/billing", "Exports", "invoice preview", 20, 15),
            ],
            "/tmp/api".to_string(),
        );

        app.query = "invoice".to_string();
        app.refresh_visible();

        assert_eq!(app.visible_ids(), vec!["2".to_string()]);
    }

    #[test]
    fn app_query_matches_ignoring_punctuation() {
        let mut app = App::new(
            vec![conversation(
                "1",
                "/tmp/api",
                "confirma-repair",
                "preview",
                30,
                10,
            )],
            "/tmp/api".to_string(),
        );

        app.query = "confirmarepair".to_string();
        app.refresh_visible();

        assert_eq!(app.visible_ids(), vec!["1".to_string()]);
    }

    #[test]
    fn app_filters_to_current_directory_when_cwd_mode_selected() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "Checkout bug", "fails on save", 30, 10),
                conversation("2", "/tmp/billing", "Exports", "invoice preview", 20, 15),
            ],
            "/tmp/api".to_string(),
        );

        app.filter_mode = FilterMode::CurrentDirectory;
        app.refresh_visible();

        assert_eq!(app.visible_ids(), vec!["1".to_string()]);
    }

    #[test]
    fn app_sorts_by_created_when_requested() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "B title", "preview", 30, 10),
                conversation("2", "/tmp/api", "A title", "preview", 20, 40),
            ],
            "/tmp/api".to_string(),
        );

        app.sort_mode = SortMode::Created;
        app.refresh_visible();

        assert_eq!(app.visible_ids(), vec!["2".to_string(), "1".to_string()]);
    }

    #[test]
    fn recency_bucket_marks_entries_older_than_a_week_as_stale() {
        let now = chrono::Utc::now();
        let stale_ms = (now - chrono::TimeDelta::days(10)).timestamp_millis();

        assert_eq!(recency_bucket_at(now, stale_ms), RecencyBucket::Stale);
    }

    #[test]
    fn recency_bucket_marks_recent_entries_as_recent() {
        let now = chrono::Utc::now();
        let recent_ms = (now - chrono::TimeDelta::hours(4)).timestamp_millis();

        assert_eq!(recency_bucket_at(now, recent_ms), RecencyBucket::Recent);
    }

    #[test]
    fn recency_bucket_marks_midweek_entries_as_this_week() {
        let now = chrono::Utc::now();
        let midweek_ms = (now - chrono::TimeDelta::days(5)).timestamp_millis();

        assert_eq!(recency_bucket_at(now, midweek_ms), RecencyBucket::ThisWeek);
    }

    #[test]
    fn recency_bucket_treats_invalid_timestamp_as_stale() {
        assert_eq!(
            recency_bucket_at(chrono::Utc::now(), i64::MAX),
            RecencyBucket::Stale
        );
    }

    #[test]
    fn normalize_search_text_removes_case_and_punctuation() {
        assert_eq!(
            normalize_search_text("Confirma-Repair / TEST"),
            "confirmarepairtest".to_string()
        );
    }

    #[test]
    fn renders_developer_path_with_home_prefix() {
        let Some(home_dir) = home_dir() else {
            unreachable!("HOME should be set in tests");
        };
        let path = home_dir.join("Developer").join("route53");

        assert_eq!(
            render_cwd(&path.display().to_string()),
            "~/Developer/route53"
        );
    }

    #[test]
    fn leaves_non_developer_paths_unchanged() {
        assert_eq!(render_cwd("/tmp/route53"), "/tmp/route53".to_string());
    }

    #[test]
    fn deduplicates_conversations_by_id_keeping_first_entry() {
        let conversations = deduplicate_conversations(vec![
            conversation("same-id", "/tmp/one", "first", "preview one", 30, 10),
            conversation("same-id", "/tmp/two", "second", "preview two", 20, 9),
            conversation("other-id", "/tmp/three", "third", "preview three", 10, 8),
        ]);

        assert_eq!(conversations.len(), 2);
        assert_eq!(conversations[0].display_title, "first");
        assert_eq!(conversations[0].cwd, "/tmp/one");
        assert_eq!(conversations[1].id, "other-id");
    }

    #[test]
    fn default_paths_resolve_inside_home_codex_directory() {
        let Some(home_dir) = home_dir() else {
            unreachable!("HOME should be set in tests");
        };

        assert_eq!(
            default_state_db_path(),
            home_dir.join(".codex").join("state_5.sqlite")
        );
        assert_eq!(
            default_session_index_path(),
            home_dir.join(".codex").join("session_index.jsonl")
        );
    }

    #[test]
    fn escape_clears_query_before_exiting() {
        let mut app = App::new(
            vec![conversation("1", "/tmp/api", "title", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );
        app.query = "needle".to_string();
        app.refresh_visible();

        assert!(!app.escape());
        assert_eq!(app.query, "");
    }

    #[test]
    fn escape_exits_when_query_already_empty() {
        let mut app = App::new(
            vec![conversation("1", "/tmp/api", "title", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );

        assert!(app.escape());
    }

    #[test]
    fn filter_and_sort_labels_match_expected_text() {
        assert_eq!(FilterMode::CurrentDirectory.label(), "[Cwd] All");
        assert_eq!(FilterMode::All.label(), "Cwd [All]");
        assert_eq!(SortMode::Updated.label(), "[Updated] Created");
        assert_eq!(SortMode::Created.label(), "Updated [Created]");
    }

    #[test]
    fn focus_cycles_forward_and_backward() {
        assert_eq!(FocusTarget::Search.next(), FocusTarget::Filter);
        assert_eq!(FocusTarget::Filter.next(), FocusTarget::Sort);
        assert_eq!(FocusTarget::Search.previous(), FocusTarget::Sort);
        assert_eq!(FocusTarget::Sort.previous(), FocusTarget::Filter);
    }

    #[test]
    fn option_changes_follow_focus() {
        let mut app = App::new(
            vec![conversation("1", "/tmp/api", "title", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );

        app.focus = FocusTarget::Filter;
        app.change_current_option_right();
        assert_eq!(app.filter_mode, FilterMode::CurrentDirectory);

        app.focus = FocusTarget::Sort;
        app.change_current_option_right();
        assert_eq!(app.sort_mode, SortMode::Created);

        app.change_current_option_left();
        assert_eq!(app.sort_mode, SortMode::Updated);

        app.focus = FocusTarget::Filter;
        app.change_current_option_left();
        assert_eq!(app.filter_mode, FilterMode::All);
    }

    #[test]
    fn moving_selection_wraps_when_results_exist() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "one", "preview", 30, 10),
                conversation("2", "/tmp/api", "two", "preview", 20, 9),
            ],
            "/tmp/api".to_string(),
        );

        app.move_up();
        assert_eq!(app.selected, 1);

        app.move_down();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn refresh_visible_resets_selection_when_results_shrink() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "alpha", "preview", 30, 10),
                conversation("2", "/tmp/api", "beta", "preview", 20, 9),
            ],
            "/tmp/api".to_string(),
        );
        app.selected = 5;
        app.query = "alpha".to_string();
        app.refresh_visible();

        assert_eq!(app.selected, 0);
        assert_eq!(app.visible_ids(), vec!["1".to_string()]);
    }

    #[test]
    fn query_editing_updates_results() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "alpha", "preview", 30, 10),
                conversation("2", "/tmp/api", "beta", "preview", 20, 9),
            ],
            "/tmp/api".to_string(),
        );

        app.append_query('b');
        assert_eq!(app.query, "b");
        assert_eq!(app.visible_ids(), vec!["2".to_string()]);

        app.backspace_query();
        assert_eq!(app.query, "");
        assert_eq!(app.visible_ids().len(), 2);

        app.append_query('a');
        app.clear_query();
        assert_eq!(app.query, "");
        assert_eq!(app.visible_ids().len(), 2);
    }

    #[test]
    fn selected_conversation_returns_current_row() {
        let app = App::new(
            vec![conversation("1", "/tmp/api", "alpha", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );

        let Some(conversation) = app.selected_conversation() else {
            unreachable!("selection should exist");
        };

        assert_eq!(conversation.id, "1");
    }

    #[test]
    fn load_thread_names_returns_empty_when_file_is_missing() {
        let temp_dir = TestDir::new("missing-session-index");
        let thread_names = load_thread_names(&temp_dir.path().join("session_index.jsonl"));
        let Ok(thread_names) = thread_names else {
            unreachable!("missing file should be allowed");
        };

        assert!(thread_names.is_empty());
    }

    #[test]
    fn load_thread_names_ignores_invalid_and_blank_entries() {
        let temp_dir = TestDir::new("session-index");
        let session_index_path = temp_dir.path().join("session_index.jsonl");
        write_file(
            &session_index_path,
            concat!(
                "{\"id\":\"thread-1\",\"thread_name\":\"Woodpecker\"}\n",
                "not json\n",
                "{\"id\":\"thread-2\",\"thread_name\":\"   \"}\n",
                "{\"id\":\"thread-3\"}\n"
            ),
        );

        let names = load_thread_names(&session_index_path);
        let Ok(names) = names else {
            unreachable!("session index should load");
        };

        assert_eq!(names.len(), 1);
        assert_eq!(names.get("thread-1"), Some(&"Woodpecker".to_string()));
    }

    #[test]
    fn load_conversations_uses_thread_names_and_deduplicates_ids() {
        let temp_dir = TestDir::new("conversations");
        let db_path = temp_dir.path().join("state.sqlite");
        let session_index_path = temp_dir.path().join("session_index.jsonl");
        write_file(
            &session_index_path,
            "{\"id\":\"thread-1\",\"thread_name\":\"Woodpecker\"}\n",
        );
        seed_threads_db(&db_path);

        let conversations = load_conversations(&db_path, &session_index_path);
        let Ok(conversations) = conversations else {
            unreachable!("conversations should load");
        };

        assert_eq!(conversations.len(), 2);
        assert_eq!(conversations[0].id, "thread-1");
        assert_eq!(conversations[0].display_title, "Woodpecker");
        assert_eq!(conversations[0].preview, "first preview");
        assert_eq!(conversations[1].display_title, "thread-2");
    }

    #[test]
    fn handle_key_event_ignores_non_press_events() {
        let mut app = App::new(
            vec![conversation("1", "/tmp/api", "alpha", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );

        let result = handle_key_event(
            &mut app,
            KeyEvent::new_with_kind(KeyCode::Char('x'), KeyModifiers::NONE, KeyEventKind::Repeat),
        );
        let Ok(result) = result else {
            unreachable!("repeat events should be ignored");
        };

        assert!(result.is_none());
        assert_eq!(app.query, "");
    }

    #[test]
    fn handle_key_event_updates_app_state_and_returns_selection() {
        let mut app = App::new(
            vec![
                conversation("1", "/tmp/api", "alpha", "preview", 30, 10),
                conversation("2", "/tmp/api", "beta", "preview", 20, 9),
            ],
            "/tmp/api".to_string(),
        );

        let key_result = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        );
        let Ok(key_result) = key_result else {
            unreachable!("char input should succeed");
        };
        assert!(key_result.is_none());
        assert_eq!(app.query, "b");

        let key_result = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        let Ok(key_result) = key_result else {
            unreachable!("backspace should succeed");
        };
        assert!(key_result.is_none());
        assert_eq!(app.query, "");

        let key_result =
            handle_key_event(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Ok(key_result) = key_result else {
            unreachable!("tab should succeed");
        };
        assert!(key_result.is_none());
        assert_eq!(app.focus, FocusTarget::Filter);

        let key_result =
            handle_key_event(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let Ok(key_result) = key_result else {
            unreachable!("right should succeed");
        };
        assert!(key_result.is_none());
        assert_eq!(app.filter_mode, FilterMode::CurrentDirectory);

        let key_result = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        );
        let Ok(key_result) = key_result else {
            unreachable!("backtab should succeed");
        };
        assert!(key_result.is_none());
        assert_eq!(app.focus, FocusTarget::Search);

        let key_result = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        let Ok(key_result) = key_result else {
            unreachable!("ctrl+u should succeed");
        };
        assert!(key_result.is_none());
        assert_eq!(app.query, "");

        let selection =
            handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let Ok(selection) = selection else {
            unreachable!("enter should succeed");
        };
        let Some(selection) = selection else {
            unreachable!("enter should select the current conversation");
        };
        assert_eq!(selection.id, "1");
    }

    #[test]
    fn handle_key_event_cancels_when_requested() {
        let mut app = App::new(
            vec![conversation("1", "/tmp/api", "alpha", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );

        let result = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        let Err(error) = result else {
            unreachable!("ctrl+c should cancel");
        };
        assert!(error.to_string().contains("selection cancelled"));
    }

    #[test]
    fn handle_key_event_esc_clears_then_cancels() {
        let mut app = App::new(
            vec![conversation("1", "/tmp/api", "alpha", "preview", 30, 10)],
            "/tmp/api".to_string(),
        );
        app.query = "alpha".to_string();

        let result = handle_key_event(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let Ok(result) = result else {
            unreachable!("first esc should clear search");
        };
        assert!(result.is_none());
        assert_eq!(app.query, "");

        let result = handle_key_event(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let Err(error) = result else {
            unreachable!("second esc should cancel");
        };
        assert!(error.to_string().contains("selection cancelled"));
    }

    #[test]
    fn render_shows_search_controls_and_results_table() {
        let app = App::new(
            vec![conversation(
                "thread-1",
                "/tmp/api",
                "alpha",
                "preview text",
                30,
                10,
            )],
            "/tmp/api".to_string(),
        );
        let screen = render_to_string(&app);

        assert!(screen.contains("Resume a previous session"));
        assert!(screen.contains("Type to search"));
        assert!(screen.contains("Filter: "));
        assert!(screen.contains("Sort: "));
        assert!(screen.contains("Conversation"));
        assert!(screen.contains("Excerpt"));
        assert!(screen.contains("alpha"));
        assert!(screen.contains("preview text"));
    }

    #[test]
    fn render_shows_empty_state_when_no_conversations_match() {
        let mut app = App::new(
            vec![conversation(
                "thread-1",
                "/tmp/api",
                "alpha",
                "preview text",
                30,
                10,
            )],
            "/tmp/api".to_string(),
        );
        app.query = "missing".to_string();
        app.refresh_visible();
        let screen = render_to_string(&app);

        assert!(screen.contains("No conversations match the current search/filter."));
    }

    #[test]
    fn resume_conversation_dry_run_and_subprocess_paths_work() {
        let temp_dir = TestDir::new("resume");
        let success_script = temp_dir.path().join("resume-ok.sh");
        write_executable(
            &success_script,
            "#!/bin/sh\n[ \"$1\" = \"resume\" ] && [ \"$2\" = \"thread-1\" ]\n",
        );

        let result = resume_conversation(&success_script.display().to_string(), "thread-1", false);
        assert!(result.is_ok());

        let dry_run = resume_conversation(&success_script.display().to_string(), "thread-1", true);
        assert!(dry_run.is_ok());
    }

    #[test]
    fn resume_conversation_reports_non_zero_exit_status() {
        let temp_dir = TestDir::new("resume-fail");
        let fail_script = temp_dir.path().join("resume-fail.sh");
        write_executable(&fail_script, "#!/bin/sh\nexit 7\n");

        let result = resume_conversation(&fail_script.display().to_string(), "thread-1", false);
        let Err(error) = result else {
            unreachable!("non-zero exit should fail");
        };

        assert!(error.to_string().contains("exited with"));
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(140, 20);
        let mut terminal = Terminal::new(backend).unwrap_or_else(|_| {
            unreachable!("test terminal should initialize");
        });

        let draw_result = terminal.draw(|frame| render(frame, app));
        assert!(draw_result.is_ok());

        buffer_to_string(terminal.backend().buffer())
    }

    fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
        let width = usize::from(buffer.area.width);

        buffer
            .content
            .chunks(width)
            .map(|cells| {
                cells
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn seed_threads_db(db_path: &Path) {
        let connection_result = Connection::open(db_path);
        let Ok(connection) = connection_result else {
            unreachable!("sqlite database should open");
        };

        let schema_result = connection.execute_batch(
            "create table threads (
                id text not null,
                cwd text not null,
                title text,
                preview text,
                first_user_message text,
                recency_at_ms integer not null,
                updated_at_ms integer not null,
                created_at_ms integer not null,
                created_at integer not null,
                archived integer not null
            );",
        );
        assert!(schema_result.is_ok());

        let first_insert = connection.execute(
            "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                "thread-1",
                "/tmp/api",
                "ignored local title",
                "first preview",
                "first message",
                2_000_i64,
                1_500_i64,
                1_000_i64,
                1_i64,
                0_i64,
            ),
        );
        assert!(first_insert.is_ok());

        let duplicate_insert = connection.execute(
            "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                "thread-1",
                "/tmp/older",
                "older duplicate",
                "older preview",
                "older message",
                1_000_i64,
                900_i64,
                800_i64,
                1_i64,
                0_i64,
            ),
        );
        assert!(duplicate_insert.is_ok());

        let second_insert = connection.execute(
            "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                "thread-2",
                "/tmp/billing",
                "billing prompt",
                "",
                "second message",
                0_i64,
                500_i64,
                400_i64,
                1_i64,
                0_i64,
            ),
        );
        assert!(second_insert.is_ok());
    }

    fn write_file(path: &Path, contents: &str) {
        let write_result = fs::write(path, contents);
        assert!(write_result.is_ok());
    }

    fn write_executable(path: &Path, contents: &str) {
        write_file(path, contents);

        let metadata_result = fs::metadata(path);
        let Ok(metadata) = metadata_result else {
            unreachable!("script metadata should exist");
        };
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);

        let permissions_result = fs::set_permissions(path, permissions);
        assert!(permissions_result.is_ok());
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0_u128, |duration| duration.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "cdx-tests-{prefix}-{}-{}",
                std::process::id(),
                timestamp
            ));
            let create_result = fs::create_dir_all(&path);
            assert!(create_result.is_ok());

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _remove_result = fs::remove_dir_all(&self.path);
        }
    }

    fn conversation(
        id: &str,
        cwd: &str,
        title: &str,
        preview: &str,
        updated_at_ms: i64,
        created_at_ms: i64,
    ) -> Conversation {
        Conversation {
            id: id.to_string(),
            cwd: cwd.to_string(),
            rendered_cwd: render_cwd(cwd),
            display_title: title.to_string(),
            preview: preview.to_string(),
            updated_at_ms,
            created_at_ms,
        }
    }
}
