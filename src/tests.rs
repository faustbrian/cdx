use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use rusqlite::Connection;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

use super::coverage_excluded::handle_key_event;
use super::{
    App, ColumnPreset, ControlsVariant, Conversation, FilterMode, FocusTarget, PREVIEW_LIMIT,
    RecencyBucket, RowMode, RunConfig, SortMode, SubagentMode, ThreadRow, column_exists,
    compact_filter_label, compact_sort_label, compact_subagent_label, controls_variant_for_width,
    controls_width, deduplicate_conversations, default_session_index_path,
    default_session_index_path_from_home, default_state_db_path, default_state_db_path_from_home,
    detect_subagent_thread_ids, display_title, effective_updated_at_ms, excerpt_column_width,
    excerpt_lines, finish_draw, format_actual_time, format_relative_delta, format_relative_time,
    format_relative_time_at, home_dir, init_terminal_session, load_conversations,
    load_conversations_with_options, load_thread_names, load_thread_query_support, next_thread_row,
    normalize_search_text, preview_text, recency_bucket_at, recency_bucket_from_delta, render,
    render_cwd, render_cwd_with_home, restore_terminal_session, resume_conversation, run_default,
    run_picker_loop, run_with, select_conversation, select_conversation_with_session,
    table_columns, table_exists, table_row, thread_row_from_sql_row, tight_filter_label,
    tight_sort_label, tight_subagent_label, truncate_to_width, with_terminal_session,
    write_dry_run_output,
};

fn run_picker_app<FSelect, FResume>(
    conversations: Vec<Conversation>,
    current_cwd: String,
    codex_bin: &str,
    dry_run: bool,
    select: FSelect,
    resume: FResume,
) -> anyhow::Result<()>
where
    FSelect: FnOnce(&mut App) -> anyhow::Result<Conversation>,
    FResume: FnOnce(&str, &str, bool) -> anyhow::Result<()>,
{
    if conversations.is_empty() {
        anyhow::bail!("no Codex conversations found");
    }

    let mut app = App::new(conversations, current_cwd);
    let selected = select(&mut app)?;
    resume(codex_bin, selected.id(), dry_run)
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("write failed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

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
fn returns_none_when_all_timestamps_are_missing() {
    assert_eq!(effective_updated_at_ms(0, 0), None);
}

#[test]
fn conversation_uses_preview_or_first_user_message() {
    let conversation = Conversation::try_from_row(
        ThreadRow {
            id: "thread-1".to_string(),
            cwd: "/tmp/example".to_string(),
            title: None,
            preview: None,
            first_user_message: Some("hello\nworld".to_string()),
            recency_at_ms: 10,
            updated_at_ms: 5,
            created_at_ms: 8,
            source: None,
            thread_source: None,
        },
        Some("thread-name"),
        false,
    )
    .unwrap_or_else(|_| unreachable!("thread should normalize"));

    assert_eq!(conversation.display_title, "thread-name");
    assert_eq!(conversation.id(), "thread-1");
    assert_eq!(conversation.preview, "hello world");
    assert_eq!(conversation.updated_at_ms, 10);
    assert_eq!(conversation.created_at_ms, 10);
    assert_eq!(conversation.rendered_cwd, "/tmp/example");
}

#[test]
fn conversation_errors_when_no_usable_timestamp_exists() {
    let error = Conversation::try_from_row(
        ThreadRow {
            id: "thread-1".to_string(),
            cwd: "/tmp/example".to_string(),
            title: None,
            preview: None,
            first_user_message: Some("hello".to_string()),
            recency_at_ms: 0,
            updated_at_ms: 0,
            created_at_ms: 0,
            source: None,
            thread_source: None,
        },
        Some("thread-name"),
        false,
    )
    .err()
    .unwrap_or_else(|| unreachable!("missing timestamps should fail"));

    assert!(error.to_string().contains("missing a usable timestamp"));
}

#[test]
fn thread_row_marks_thread_source_as_subagent() {
    let row = ThreadRow {
        id: "thread-1".to_string(),
        cwd: "/tmp/example".to_string(),
        title: None,
        preview: None,
        first_user_message: None,
        recency_at_ms: 1,
        updated_at_ms: 1,
        created_at_ms: 1,
        source: None,
        thread_source: Some("subagent".to_string()),
    };

    assert!(row.is_subagent(&HashSet::new()));
}

#[test]
fn thread_row_marks_spawned_thread_id_as_subagent() {
    let row = ThreadRow {
        id: "thread-1".to_string(),
        cwd: "/tmp/example".to_string(),
        title: None,
        preview: None,
        first_user_message: None,
        recency_at_ms: 1,
        updated_at_ms: 1,
        created_at_ms: 1,
        source: None,
        thread_source: None,
    };

    let subagent_thread_ids = HashSet::from(["thread-1".to_string()]);

    assert!(row.is_subagent(&subagent_thread_ids));
}

#[test]
fn thread_row_marks_source_payload_as_subagent() {
    let row = ThreadRow {
        id: "thread-1".to_string(),
        cwd: "/tmp/example".to_string(),
        title: None,
        preview: None,
        first_user_message: None,
        recency_at_ms: 1,
        updated_at_ms: 1,
        created_at_ms: 1,
        source: Some(r#"{"subagent":{"thread_spawn":{"depth":1}}}"#.to_string()),
        thread_source: None,
    };

    assert!(row.is_subagent(&HashSet::new()));
}

#[test]
fn thread_row_ignores_non_subagent_markers() {
    let row = ThreadRow {
        id: "thread-1".to_string(),
        cwd: "/tmp/example".to_string(),
        title: None,
        preview: None,
        first_user_message: None,
        recency_at_ms: 1,
        updated_at_ms: 1,
        created_at_ms: 1,
        source: Some(r#"{"agent":{"thread_spawn":{"depth":1}}}"#.to_string()),
        thread_source: Some("primary".to_string()),
    };

    assert!(!row.is_subagent(&HashSet::new()));
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
fn format_relative_delta_covers_all_display_ranges() {
    assert_eq!(
        format_relative_delta(chrono::TimeDelta::seconds(10)),
        "just now"
    );
    assert_eq!(
        format_relative_delta(chrono::TimeDelta::minutes(7)),
        "7m ago"
    );
    assert_eq!(format_relative_delta(chrono::TimeDelta::hours(9)), "9h ago");
    assert_eq!(format_relative_delta(chrono::TimeDelta::days(3)), "3d ago");
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
fn app_sorts_equal_timestamps_by_title() {
    let mut updated_app = App::new(
        vec![
            conversation("1", "/tmp/api", "B title", "preview", 30, 10),
            conversation("2", "/tmp/api", "A title", "preview", 30, 9),
        ],
        "/tmp/api".to_string(),
    );
    updated_app.refresh_visible();
    assert_eq!(
        updated_app.visible_ids(),
        vec!["2".to_string(), "1".to_string()]
    );

    let mut created_app = App::new(
        vec![
            conversation("1", "/tmp/api", "B title", "preview", 30, 10),
            conversation("2", "/tmp/api", "A title", "preview", 20, 10),
        ],
        "/tmp/api".to_string(),
    );
    created_app.sort_mode = SortMode::Created;
    created_app.refresh_visible();

    assert_eq!(
        created_app.visible_ids(),
        vec!["2".to_string(), "1".to_string()]
    );
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
fn recency_bucket_from_delta_covers_all_ranges() {
    assert_eq!(
        recency_bucket_from_delta(chrono::TimeDelta::hours(4)),
        RecencyBucket::Recent
    );
    assert_eq!(
        recency_bucket_from_delta(chrono::TimeDelta::days(5)),
        RecencyBucket::ThisWeek
    );
    assert_eq!(
        recency_bucket_from_delta(chrono::TimeDelta::days(10)),
        RecencyBucket::Stale
    );
}

#[test]
fn recency_bucket_styles_are_colored_by_age() {
    assert_eq!(
        RecencyBucket::Recent.style().fg,
        Some(ratatui::style::Color::Cyan)
    );
    assert_eq!(
        RecencyBucket::ThisWeek.style().fg,
        Some(ratatui::style::Color::Yellow)
    );
    assert_eq!(
        RecencyBucket::Stale.style().fg,
        Some(ratatui::style::Color::LightRed)
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
    let home_dir = home_dir().unwrap_or_else(|| unreachable!("HOME should be set in tests"));
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
fn renders_developer_root_with_home_prefix() {
    let home_dir = home_dir().unwrap_or_else(|| unreachable!("HOME should be set in tests"));

    assert_eq!(
        render_cwd(&home_dir.join("Developer").display().to_string()),
        "~/Developer".to_string()
    );
}

#[test]
fn leaves_paths_unchanged_when_home_directory_is_unavailable() {
    assert_eq!(
        render_cwd_with_home("/tmp/route53", None),
        "/tmp/route53".to_string()
    );
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
    let home_dir = home_dir().unwrap_or_else(|| unreachable!("HOME should be set in tests"));

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
fn default_paths_fall_back_to_tilde_when_home_is_missing() {
    assert_eq!(
        default_state_db_path_from_home(None),
        PathBuf::from("~").join(".codex").join("state_5.sqlite")
    );
    assert_eq!(
        default_session_index_path_from_home(None),
        PathBuf::from("~")
            .join(".codex")
            .join("session_index.jsonl")
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
    assert_eq!(SubagentMode::Hide.label(), "[Primary] All");
    assert_eq!(SubagentMode::Show.label(), "Primary [All]");
    assert_eq!(SortMode::Updated.label(), "[Updated] Created");
    assert_eq!(SortMode::Created.label(), "Updated [Created]");
}

#[test]
fn filter_sort_and_focus_cycle_in_both_directions() {
    assert_eq!(FilterMode::CurrentDirectory.next(), FilterMode::All);
    assert_eq!(FilterMode::All.previous(), FilterMode::CurrentDirectory);

    assert_eq!(SubagentMode::Hide.next(), SubagentMode::Show);
    assert_eq!(SubagentMode::Show.previous(), SubagentMode::Hide);

    assert_eq!(SortMode::Updated.next(), SortMode::Created);
    assert_eq!(SortMode::Created.previous(), SortMode::Updated);

    assert_eq!(FocusTarget::Search.next(), FocusTarget::Filter);
    assert_eq!(FocusTarget::Filter.next(), FocusTarget::Subagents);
    assert_eq!(FocusTarget::Subagents.next(), FocusTarget::Sort);
    assert_eq!(FocusTarget::Sort.next(), FocusTarget::Search);

    assert_eq!(FocusTarget::Search.previous(), FocusTarget::Sort);
    assert_eq!(FocusTarget::Filter.previous(), FocusTarget::Search);
    assert_eq!(FocusTarget::Subagents.previous(), FocusTarget::Filter);
    assert_eq!(FocusTarget::Sort.previous(), FocusTarget::Subagents);
}

#[test]
fn focus_cycles_forward_and_backward() {
    assert_eq!(FocusTarget::Search.next(), FocusTarget::Filter);
    assert_eq!(FocusTarget::Filter.next(), FocusTarget::Subagents);
    assert_eq!(FocusTarget::Subagents.next(), FocusTarget::Sort);
    assert_eq!(FocusTarget::Sort.next(), FocusTarget::Search);
    assert_eq!(FocusTarget::Search.previous(), FocusTarget::Sort);
    assert_eq!(FocusTarget::Sort.previous(), FocusTarget::Subagents);
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

    app.focus = FocusTarget::Subagents;
    app.change_current_option_right();
    assert_eq!(app.subagent_mode, SubagentMode::Show);

    app.change_current_option_left();
    assert_eq!(app.subagent_mode, SubagentMode::Hide);

    app.focus = FocusTarget::Sort;
    app.change_current_option_right();
    assert_eq!(app.sort_mode, SortMode::Created);

    app.change_current_option_left();
    assert_eq!(app.sort_mode, SortMode::Updated);

    app.focus = FocusTarget::Filter;
    app.change_current_option_left();
    assert_eq!(app.filter_mode, FilterMode::All);

    app.focus = FocusTarget::Search;
    app.change_current_option_right();
    app.change_current_option_left();
    assert_eq!(app.filter_mode, FilterMode::All);
    assert_eq!(app.sort_mode, SortMode::Updated);
}

#[test]
fn app_hides_subagents_by_default_and_can_show_them() {
    let mut child = conversation("child", "/tmp/api", "child", "preview", 20, 10);
    child.is_subagent = true;

    let mut app = App::new(
        vec![
            conversation("parent", "/tmp/api", "parent", "preview", 30, 10),
            child,
        ],
        "/tmp/api".to_string(),
    );

    assert_eq!(app.visible_ids(), vec!["parent".to_string()]);

    app.focus = FocusTarget::Subagents;
    app.change_current_option_right();

    assert_eq!(
        app.visible_ids(),
        vec!["parent".to_string(), "child".to_string()]
    );
}

#[test]
fn view_toggles_update_app_preferences() {
    let mut app = App::new(
        vec![conversation("1", "/tmp/api", "title", "preview", 30, 10)],
        "/tmp/api".to_string(),
    );

    assert_eq!(app.column_preset, ColumnPreset::Core);
    assert_eq!(app.row_mode, RowMode::Compact);

    app.toggle_column_preset();
    app.toggle_row_mode();

    assert_eq!(app.column_preset, ColumnPreset::Full);
    assert_eq!(app.row_mode, RowMode::Comfortable);
}

#[test]
fn view_toggles_round_trip_to_defaults() {
    let mut app = App::new(
        vec![conversation("1", "/tmp/api", "title", "preview", 30, 10)],
        "/tmp/api".to_string(),
    );

    app.toggle_column_preset();
    app.toggle_column_preset();
    app.toggle_row_mode();
    app.toggle_row_mode();

    assert_eq!(app.column_preset, ColumnPreset::Core);
    assert_eq!(app.row_mode, RowMode::Compact);
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
fn moving_selection_on_empty_results_is_a_no_op() {
    let mut app = App::new(Vec::new(), "/tmp/api".to_string());

    app.move_down();
    app.move_up();

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

    let conversation = app
        .selected_conversation()
        .unwrap_or_else(|| unreachable!("selection should exist"));

    assert_eq!(conversation.id, "1");
}

#[test]
fn selected_conversation_returns_none_when_results_are_empty() {
    let app = App::new(Vec::new(), "/tmp/api".to_string());

    assert!(app.selected_conversation().is_none());
}

#[test]
fn load_thread_names_returns_empty_when_file_is_missing() {
    let temp_dir = TestDir::new("missing-session-index");
    let thread_names = load_thread_names(&temp_dir.path().join("session_index.jsonl"))
        .unwrap_or_else(|_| unreachable!("missing file should be allowed"));

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

    let names = load_thread_names(&session_index_path)
        .unwrap_or_else(|_| unreachable!("session index should load"));

    assert_eq!(names.len(), 1);
    assert_eq!(names.get("thread-1"), Some(&"Woodpecker".to_string()));
}

#[test]
fn load_thread_names_returns_context_for_directory_paths() {
    let temp_dir = TestDir::new("session-index-directory");
    let result = load_thread_names(temp_dir.path());
    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("directory paths should fail"));

    assert!(error.to_string().contains("session-index-directory"));
}

#[test]
fn load_thread_names_returns_context_for_permission_errors() {
    let temp_dir = TestDir::new("session-index-permissions");
    let session_index_path = temp_dir.path().join("session_index.jsonl");
    write_file(
        &session_index_path,
        "{\"id\":\"thread-1\",\"thread_name\":\"Woodpecker\"}\n",
    );

    let metadata = fs::metadata(&session_index_path)
        .unwrap_or_else(|_| unreachable!("session index metadata should exist"));
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&session_index_path, permissions)
        .unwrap_or_else(|_| unreachable!("permissions should update"));

    let error = load_thread_names(&session_index_path)
        .err()
        .unwrap_or_else(|| unreachable!("permission errors should fail"));

    assert!(error.to_string().contains("failed to open"));
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

    let conversations = load_conversations(&db_path, &session_index_path)
        .unwrap_or_else(|_| unreachable!("conversations should load"));

    assert_eq!(conversations.len(), 2);
    assert_eq!(conversations[0].id, "thread-1");
    assert_eq!(conversations[0].display_title, "Woodpecker");
    assert_eq!(conversations[0].preview, "first preview");
    assert_eq!(conversations[1].display_title, "thread-2");
}

#[test]
fn load_conversations_excludes_spawned_subagents_by_default() {
    let temp_dir = TestDir::new("subagent-filter-default");
    let db_path = temp_dir.path().join("state.sqlite");
    seed_threads_db_with_subagent(&db_path);

    let conversations = load_conversations(&db_path, &temp_dir.path().join("missing.jsonl"))
        .unwrap_or_else(|_| unreachable!("conversations should load"));

    assert_eq!(
        conversations
            .iter()
            .map(|conversation| conversation.id.as_str())
            .collect::<Vec<_>>(),
        vec!["thread-parent", "thread-plain"]
    );
}

#[test]
fn load_conversations_with_options_can_include_spawned_subagents() {
    let temp_dir = TestDir::new("subagent-filter-include");
    let db_path = temp_dir.path().join("state.sqlite");
    seed_threads_db_with_subagent(&db_path);

    let conversations =
        load_conversations_with_options(&db_path, &temp_dir.path().join("missing.jsonl"), true)
            .unwrap_or_else(|_| unreachable!("conversations should load"));

    assert_eq!(
        conversations
            .iter()
            .map(|conversation| conversation.id.as_str())
            .collect::<Vec<_>>(),
        vec!["thread-parent", "thread-child", "thread-plain"]
    );
}

#[test]
fn table_and_column_exists_report_presence_and_absence() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
            "create table threads (
                id text not null,
                source text
            );
            create table thread_spawn_edges (
                parent_thread_id text not null,
                child_thread_id text not null,
                status text not null
            );",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));

    assert!(
        table_exists(&connection, "threads")
            .unwrap_or_else(|_| unreachable!("table existence should query"))
    );
    assert!(
        table_exists(&connection, "thread_spawn_edges")
            .unwrap_or_else(|_| unreachable!("table existence should query"))
    );
    assert!(
        !table_exists(&connection, "missing_table")
            .unwrap_or_else(|_| unreachable!("table existence should query"))
    );
    assert!(
        column_exists(&connection, "threads", "source")
            .unwrap_or_else(|_| unreachable!("column existence should query"))
    );
    assert!(
        !column_exists(&connection, "threads", "thread_source")
            .unwrap_or_else(|_| unreachable!("column existence should query"))
    );
}

#[test]
fn column_exists_returns_false_for_invalid_table_names() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    assert!(
        !column_exists(&connection, "threads)", "source")
            .unwrap_or_else(|_| unreachable!("invalid table names should not fail"))
    );
}

#[test]
fn table_exists_propagates_authorizer_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "sqlite_master",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }))
        .unwrap_or_else(|_| unreachable!("authorizer should install"));

    assert!(table_exists(&connection, "threads").is_err());
}

#[test]
fn load_thread_query_support_propagates_subagent_lookup_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
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
            );
            create table thread_spawn_edges (
                parent_thread_id text not null,
                child_thread_id text not null,
                status text not null
            );",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));
    connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "thread_spawn_edges",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }))
        .unwrap_or_else(|_| unreachable!("authorizer should install"));

    assert!(load_thread_query_support(&connection).is_err());
}

#[test]
fn load_thread_query_support_propagates_first_column_probe_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
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
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));

    connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Pragma {
                pragma_name: "table_info",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }))
        .unwrap_or_else(|_| unreachable!("authorizer should install"));

    assert!(load_thread_query_support(&connection).is_err());
}

#[test]
fn load_thread_query_support_propagates_second_column_probe_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));
    let pragma_calls = Arc::new(Mutex::new(0_usize));
    let pragma_calls_capture = Arc::clone(&pragma_calls);

    connection
        .execute_batch(
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
                archived integer not null,
                source text
            );",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));

    connection
        .authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::Pragma {
                pragma_name: "table_info",
                ..
            } => {
                let mut calls = pragma_calls_capture
                    .lock()
                    .unwrap_or_else(|_| unreachable!("pragma counter should lock"));
                *calls += 1;
                if *calls == 2 {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            _ => Authorization::Allow,
        }))
        .unwrap_or_else(|_| unreachable!("authorizer should install"));

    assert!(load_thread_query_support(&connection).is_err());
}

#[test]
fn detect_subagent_thread_ids_propagates_decode_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
            "create table thread_spawn_edges (
                parent_thread_id text not null,
                child_thread_id integer not null,
                status text not null
            );
            insert into thread_spawn_edges (parent_thread_id, child_thread_id, status)
            values ('parent', 1, 'closed');",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));

    let error = detect_subagent_thread_ids(&connection)
        .err()
        .unwrap_or_else(|| unreachable!("decode errors should propagate"));

    assert!(error.to_string().contains("failed to decode"));
}

#[test]
fn detect_subagent_thread_ids_propagates_table_exists_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "sqlite_master",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }))
        .unwrap_or_else(|_| unreachable!("authorizer should install"));

    assert!(detect_subagent_thread_ids(&connection).is_err());
}

#[test]
fn detect_subagent_thread_ids_propagates_authorizer_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
            "create table thread_spawn_edges (
                parent_thread_id text not null,
                child_thread_id text not null,
                status text not null
            );",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));
    connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "thread_spawn_edges",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }))
        .unwrap_or_else(|_| unreachable!("authorizer should install"));

    assert!(detect_subagent_thread_ids(&connection).is_err());
}

#[test]
fn load_conversations_propagates_session_index_errors() {
    let temp_dir = TestDir::new("load-index-error");
    let db_path = temp_dir.path().join("state.sqlite");
    seed_threads_db(&db_path);

    let error = load_conversations(&db_path, temp_dir.path())
        .err()
        .unwrap_or_else(|| unreachable!("directory session index should fail"));

    assert!(error.to_string().contains("failed to read"));
}

#[test]
fn load_conversations_propagates_row_conversion_errors() {
    let temp_dir = TestDir::new("load-row-error");
    let db_path = temp_dir.path().join("state.sqlite");
    let connection =
        Connection::open(&db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
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
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));
    connection
        .execute(
            "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                "thread-1", "/tmp/api", "broken", "preview", "message", 0_i64, 0_i64, 0_i64, 0_i64,
                0_i64,
            ),
        )
        .unwrap_or_else(|_| unreachable!("row should insert"));

    let error = load_conversations(&db_path, &temp_dir.path().join("missing.jsonl"))
        .err()
        .unwrap_or_else(|| unreachable!("invalid rows should fail"));

    assert!(error.to_string().contains("missing a usable timestamp"));
}

#[test]
fn load_conversations_propagates_query_row_decode_errors() {
    let temp_dir = TestDir::new("load-query-map-error");
    let db_path = temp_dir.path().join("state.sqlite");
    let connection =
        Connection::open(&db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
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
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));
    connection
        .execute(
            "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                "thread-1",
                "/tmp/api",
                "broken",
                "preview",
                "message",
                "not-an-integer",
                0_i64,
                0_i64,
                0_i64,
                0_i64,
            ),
        )
        .unwrap_or_else(|_| unreachable!("row should insert"));

    let error = load_conversations(&db_path, &temp_dir.path().join("missing.jsonl"))
        .err()
        .unwrap_or_else(|| unreachable!("invalid row types should fail"));

    assert!(error.to_string().contains("failed to query thread rows"));
}

#[test]
fn load_conversations_propagates_thread_query_support_errors() {
    let temp_dir = TestDir::new("load-query-support-error");
    let db_path = temp_dir.path().join("state.sqlite");
    let connection =
        Connection::open(&db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));

    connection
        .execute_batch(
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
            );
            create table thread_spawn_edges (
                parent_thread_id text not null,
                child_thread_id integer not null,
                status text not null
            );
            insert into thread_spawn_edges (parent_thread_id, child_thread_id, status)
            values ('parent', 1, 'closed');",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));

    let error = load_conversations(&db_path, &temp_dir.path().join("missing.jsonl"))
        .err()
        .unwrap_or_else(|| unreachable!("thread query support errors should propagate"));

    assert!(error.to_string().contains("failed to decode"));
}

#[test]
fn thread_row_from_sql_row_propagates_decode_failures_for_each_column() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));
    let cases = [
        "select 1, '/tmp/api', null, null, null, 1, 1, 1",
        "select 'thread-1', 1, null, null, null, 1, 1, 1",
        "select 'thread-1', '/tmp/api', 1, null, null, 1, 1, 1",
        "select 'thread-1', '/tmp/api', null, 1, null, 1, 1, 1",
        "select 'thread-1', '/tmp/api', null, null, 1, 1, 1, 1",
        "select 'thread-1', '/tmp/api', null, null, null, 'bad', 1, 1",
        "select 'thread-1', '/tmp/api', null, null, null, 1, 'bad', 1",
        "select 'thread-1', '/tmp/api', null, null, null, 1, 1, 'bad'",
        "select 'thread-1', '/tmp/api', null, null, null, 1, 1, 1, 1",
        "select 'thread-1', '/tmp/api', null, null, null, 1, 1, 1, null, 1",
    ];

    for sql in cases {
        let error = connection
            .query_row(sql, [], thread_row_from_sql_row)
            .err()
            .unwrap_or_else(|| unreachable!("malformed row should fail"));

        assert!(error.to_string().contains("Invalid column type"));
    }
}

#[test]
fn next_thread_row_returns_rows_then_none() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));
    connection
        .execute_batch(
            "create table threads (
                id text not null,
                cwd text not null,
                title text,
                preview text,
                first_user_message text,
                recency_at_ms integer not null,
                updated_at_ms integer not null,
                created_at_ms integer not null
            );",
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));
    connection
        .execute(
            "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                "thread-1", "/tmp/api", "alpha", "preview", "message", 3_i64, 2_i64, 1_i64,
            ),
        )
        .unwrap_or_else(|_| unreachable!("row should insert"));

    let mut statement = connection
        .prepare(
            "select id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms
             from threads",
        )
        .unwrap_or_else(|_| unreachable!("query should prepare"));
    let mut rows = statement.raw_query();

    let first = next_thread_row(&mut rows)
        .unwrap_or_else(|_| unreachable!("first row should decode"))
        .unwrap_or_else(|| unreachable!("first row should exist"));
    assert_eq!(first.id, "thread-1");

    let second =
        next_thread_row(&mut rows).unwrap_or_else(|_| unreachable!("empty rows should not fail"));
    assert!(second.is_none());
}

#[test]
fn next_thread_row_propagates_query_step_errors() {
    let connection = Connection::open_in_memory()
        .unwrap_or_else(|_| unreachable!("sqlite database should open"));
    let mut statement = connection
        .prepare(
            "with recursive counter(value) as (
                values(0)
                union all
                select value + 1 from counter where value < 50000000
             )
             select
                'thread-1',
                '/tmp/api',
                'alpha',
                'preview',
                'message',
                3,
                2,
                1
             from counter
             where value = 50000000",
        )
        .unwrap_or_else(|_| unreachable!("query should prepare"));
    let mut rows = statement.raw_query();
    let interrupt_handle = connection.get_interrupt_handle();
    let interrupter = thread::spawn(move || {
        thread::sleep(Duration::from_millis(1));
        interrupt_handle.interrupt();
    });

    let error = next_thread_row(&mut rows)
        .err()
        .unwrap_or_else(|| unreachable!("interrupts should propagate as query errors"));
    interrupter
        .join()
        .unwrap_or_else(|_| unreachable!("interrupt thread should join cleanly"));
    assert!(error.to_string().contains("failed to query thread rows"));
}

#[test]
fn finish_draw_converts_success_and_errors() {
    finish_draw::<(), io::Error>(Ok(()))
        .unwrap_or_else(|_| unreachable!("successful draws should stay successful"));

    let error = finish_draw::<(), io::Error>(Err(io::Error::other("draw failed")))
        .err()
        .unwrap_or_else(|| unreachable!("draw errors should propagate"));
    assert!(error.to_string().contains("draw failed"));
}

#[test]
fn load_conversations_returns_empty_when_no_active_threads_exist() {
    let temp_dir = TestDir::new("empty-conversations");
    let db_path = temp_dir.path().join("state.sqlite");
    let connection =
        Connection::open(&db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));
    connection
        .execute_batch(
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
        )
        .unwrap_or_else(|_| unreachable!("schema should create"));

    let conversations = load_conversations(&db_path, &temp_dir.path().join("missing.jsonl"))
        .unwrap_or_else(|_| unreachable!("empty database should load"));

    assert!(conversations.is_empty());
}

#[test]
fn load_conversations_propagates_schema_errors() {
    let temp_dir = TestDir::new("invalid-conversations");
    let db_path = temp_dir.path().join("state.sqlite");
    let connection =
        Connection::open(&db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));
    connection
        .execute_batch("create table threads (id text not null);")
        .unwrap_or_else(|_| unreachable!("schema should create"));

    let error = load_conversations(&db_path, &temp_dir.path().join("missing.jsonl"))
        .err()
        .unwrap_or_else(|| unreachable!("invalid schema should fail"));

    assert!(error.to_string().contains("no such column"));
}

#[test]
fn load_conversations_reports_database_open_failures() {
    let temp_dir = TestDir::new("open-failure");
    let error = load_conversations(temp_dir.path(), &temp_dir.path().join("missing.jsonl"))
        .err()
        .unwrap_or_else(|| unreachable!("directory database path should fail"));

    assert!(error.to_string().contains("failed to open"));
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
    let result = result.unwrap_or_else(|_| unreachable!("repeat events should be ignored"));

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
    let key_result = key_result.unwrap_or_else(|_| unreachable!("char input should succeed"));
    assert!(key_result.is_none());
    assert_eq!(app.query, "b");

    let key_result = handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    );
    let key_result = key_result.unwrap_or_else(|_| unreachable!("backspace should succeed"));
    assert!(key_result.is_none());
    assert_eq!(app.query, "");

    let key_result = handle_key_event(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let key_result = key_result.unwrap_or_else(|_| unreachable!("tab should succeed"));
    assert!(key_result.is_none());
    assert_eq!(app.focus, FocusTarget::Filter);

    let key_result = handle_key_event(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    let key_result = key_result.unwrap_or_else(|_| unreachable!("right should succeed"));
    assert!(key_result.is_none());
    assert_eq!(app.filter_mode, FilterMode::CurrentDirectory);

    let key_result = handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
    );
    let key_result = key_result.unwrap_or_else(|_| unreachable!("space should toggle options"));
    assert!(key_result.is_none());
    assert_eq!(app.filter_mode, FilterMode::All);
    assert_eq!(app.query, "");

    let key_result = handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    let key_result = key_result.unwrap_or_else(|_| unreachable!("backtab should succeed"));
    assert!(key_result.is_none());
    assert_eq!(app.focus, FocusTarget::Search);

    let key_result = handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    let key_result = key_result.unwrap_or_else(|_| unreachable!("ctrl+u should succeed"));
    assert!(key_result.is_none());
    assert_eq!(app.query, "");

    let selection = handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let selection = selection.unwrap_or_else(|_| unreachable!("enter should succeed"));
    let selection =
        selection.unwrap_or_else(|| unreachable!("enter should select the current conversation"));
    assert_eq!(selection.id, "1");
}

#[test]
fn handle_key_event_toggles_view_preferences() {
    let mut app = App::new(
        vec![conversation("1", "/tmp/api", "alpha", "preview", 30, 10)],
        "/tmp/api".to_string(),
    );

    handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
    )
    .unwrap_or_else(|_| unreachable!("ctrl+v should succeed"));
    assert_eq!(app.column_preset, ColumnPreset::Full);

    handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
    )
    .unwrap_or_else(|_| unreachable!("ctrl+o should succeed"));
    assert_eq!(app.row_mode, RowMode::Comfortable);
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
    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("ctrl+c should cancel"));
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
    let result = result.unwrap_or_else(|_| unreachable!("first esc should clear search"));
    assert!(result.is_none());
    assert_eq!(app.query, "");

    let result = handle_key_event(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let Err(error) = result else {
        unreachable!("second esc should cancel");
    };
    assert!(error.to_string().contains("selection cancelled"));
}

#[test]
fn handle_key_event_supports_navigation_keys() {
    let mut app = App::new(
        vec![
            conversation("1", "/tmp/api", "alpha", "preview", 30, 10),
            conversation("2", "/tmp/api", "beta", "preview", 20, 9),
        ],
        "/tmp/api".to_string(),
    );

    handle_key_event(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .unwrap_or_else(|_| unreachable!("down should succeed"));
    assert_eq!(app.selected, 1);

    handle_key_event(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
        .unwrap_or_else(|_| unreachable!("up should succeed"));
    assert_eq!(app.selected, 0);

    app.focus = FocusTarget::Filter;
    handle_key_event(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .unwrap_or_else(|_| unreachable!("left should succeed"));
    assert_eq!(app.filter_mode, FilterMode::CurrentDirectory);

    app.focus = FocusTarget::Subagents;
    handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
    )
    .unwrap_or_else(|_| unreachable!("space should toggle subagent visibility"));
    assert_eq!(app.subagent_mode, SubagentMode::Show);
}

#[test]
fn handle_key_event_ignores_enter_without_visible_selection_and_unknown_keys() {
    let mut app = App::new(Vec::new(), "/tmp/api".to_string());

    let result = handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap_or_else(|_| unreachable!("enter without selection should succeed"));
    assert!(result.is_none());

    let result = handle_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT),
    )
    .unwrap_or_else(|_| unreachable!("unknown key should succeed"));
    assert!(result.is_none());

    let result = handle_key_event(&mut app, KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE))
        .unwrap_or_else(|_| unreachable!("function key should succeed"));
    assert!(result.is_none());
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
    assert!(screen.contains("Filter: Cwd [All]"));
    assert!(screen.contains("Threads: [Primary] All"));
    assert!(screen.contains("Sort: [Updated] Created"));
    assert!(screen.contains("Conversation"));
    assert!(screen.contains("Excerpt"));
    assert!(!screen.contains("Age"));
    assert!(!screen.contains("  Updated         "));
    assert!(screen.contains("alpha"));
    assert!(screen.contains("preview text"));
}

#[test]
fn render_keeps_sort_visible_on_narrow_terminals() {
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

    let screen = render_to_string_with_size(&app, 110, 20);

    assert!(screen.contains("Sort: [Updated] Created"), "{screen}");
}

#[test]
fn control_variants_select_full_compact_and_tight_layouts() {
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

    assert_eq!(controls_variant_for_width(&app, 110), ControlsVariant::Full);
    assert_eq!(
        controls_variant_for_width(&app, 86),
        ControlsVariant::Compact
    );
    assert_eq!(
        controls_variant_for_width(&app, 80),
        ControlsVariant::Compact
    );

    app.query = "search".to_string();
    assert_eq!(controls_variant_for_width(&app, 50), ControlsVariant::Tight);
    assert_eq!(
        controls_variant_for_width(&app, 30),
        ControlsVariant::Minimal
    );
}

#[test]
fn controls_width_handles_empty_input() {
    assert_eq!(controls_width(&[]), 0);
}

#[test]
fn control_variant_labels_cover_alternate_mode_values() {
    assert_eq!(
        compact_filter_label(FilterMode::CurrentDirectory),
        "[Cwd] All"
    );
    assert_eq!(compact_subagent_label(SubagentMode::Show), "Primary [All]");
    assert_eq!(compact_sort_label(SortMode::Created), "Updated [Created]");
    assert_eq!(tight_filter_label(FilterMode::CurrentDirectory), "[C]");
    assert_eq!(tight_filter_label(FilterMode::All), "[A]");
    assert_eq!(tight_subagent_label(SubagentMode::Show), "[A]");
    assert_eq!(tight_sort_label(SortMode::Created), "[C]");
}

#[test]
fn tight_controls_render_short_labels_for_alternate_modes() {
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
    app.filter_mode = FilterMode::All;
    app.subagent_mode = SubagentMode::Show;
    app.sort_mode = SortMode::Created;

    let line = ControlsVariant::Tight.line(
        &app,
        ratatui::style::Style::default(),
        ratatui::style::Style::default(),
        ratatui::style::Style::default(),
    );

    let rendered = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(ControlsVariant::Tight.width(&app), 17);
    assert_eq!(rendered, "F:[A] T:[A] S:[C]");
}

#[test]
fn minimal_controls_render_shortest_labels_for_default_modes() {
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

    let line = ControlsVariant::Minimal.line(
        &app,
        ratatui::style::Style::default(),
        ratatui::style::Style::default(),
        ratatui::style::Style::default(),
    );

    let rendered = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(ControlsVariant::Minimal.width(&app), 14);
    assert_eq!(rendered, "F[A] T[P] S[U]");
}

#[test]
fn render_can_show_full_column_set() {
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
    app.column_preset = ColumnPreset::Full;

    let screen = render_to_string(&app);

    assert!(screen.contains("Age"));
    assert!(screen.contains("Updated"));
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
fn render_shows_active_query_status_and_no_preview_placeholder() {
    let mut app = App::new(
        vec![conversation("thread-1", "/tmp/api", "alpha", "", 30, 10)],
        "/tmp/api".to_string(),
    );
    app.query = "alpha".to_string();
    app.refresh_visible();
    let screen = render_to_string(&app);

    assert!(screen.contains("alpha"));
    assert!(screen.contains("[no preview]"));
    assert!(screen.contains("1/1"));
    assert!(screen.contains("Cols: Core"));
    assert!(screen.contains("View: Compact"));
    assert!(screen.contains("Threads: Primary"));
}

#[test]
fn render_status_shows_all_threads_when_subagents_are_visible() {
    let mut child = conversation("thread-2", "/tmp/api", "child", "preview", 20, 10);
    child.is_subagent = true;

    let app = App::new_with_options(
        vec![
            conversation("thread-1", "/tmp/api", "alpha", "preview", 30, 10),
            child,
        ],
        "/tmp/api".to_string(),
        true,
    );

    let screen = render_to_string(&app);

    assert!(screen.contains("Threads: All"));
}

#[test]
fn render_shows_unknown_timestamp_when_date_formatting_fails() {
    let mut app = App::new(
        vec![conversation(
            "thread-1",
            "/tmp/api",
            "alpha",
            "preview text",
            i64::MAX,
            10,
        )],
        "/tmp/api".to_string(),
    );
    app.column_preset = ColumnPreset::Full;
    let screen = render_to_string(&app);

    assert!(screen.contains("unknown"));
}

#[test]
fn render_covers_filter_and_sort_focus_states() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );
    app.focus = FocusTarget::Filter;
    let filter_screen = render_to_string(&app);
    assert!(filter_screen.contains("Filter: "));

    app.focus = FocusTarget::Subagents;
    let subagent_screen = render_to_string(&app);
    assert!(subagent_screen.contains("Threads: "));

    app.focus = FocusTarget::Sort;
    let sort_screen = render_to_string(&app);
    assert!(sort_screen.contains("Sort: "));
}

#[test]
fn render_uses_minimal_controls_before_truncating_sort() {
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

    let screen = render_to_string_with_size(&app, 40, 20);

    assert!(screen.contains("S:[U]"), "{screen}");
}

#[test]
fn render_comfortable_mode_shows_more_excerpt_content() {
    let mut app = App::new(
        vec![conversation(
            "thread-1",
            "/tmp/api",
            "alpha",
            &"x".repeat(150),
            30,
            10,
        )],
        "/tmp/api".to_string(),
    );
    app.row_mode = RowMode::Comfortable;

    let screen = render_to_string(&app);

    assert!(screen.contains("View: Comfortable"));
    assert!(screen.matches('x').count() > 72);
}

#[test]
fn comfortable_table_row_handles_single_line_excerpt() {
    let _row = table_row(
        &conversation("thread-1", "/tmp/api", "alpha", "short excerpt", 30, 10),
        ColumnPreset::Core,
        RowMode::Comfortable,
        20,
    );
}

#[test]
fn truncate_to_width_preserves_column_width_with_ellipsis() {
    assert_eq!(truncate_to_width("abcdef", 4), "abc…");
    assert_eq!(truncate_to_width("abcdef", 1), "…");
    assert_eq!(truncate_to_width("abc", 4), "abc");
    assert_eq!(truncate_to_width("abc", 0), "");
}

#[test]
fn excerpt_column_width_tracks_layout_for_core_and_full_columns() {
    let core_width =
        excerpt_column_width(ratatui::layout::Rect::new(0, 0, 90, 10), ColumnPreset::Core);
    let full_width =
        excerpt_column_width(ratatui::layout::Rect::new(0, 0, 90, 10), ColumnPreset::Full);

    assert!(core_width > full_width);
    assert!(core_width > 0);
    assert!(full_width > 0);
}

#[test]
fn table_columns_return_expected_headers_and_widths() {
    let (_core_header, core_widths) = table_columns(ColumnPreset::Core);
    let (_full_header, full_widths) = table_columns(ColumnPreset::Full);

    assert_eq!(core_widths.len(), 3);
    assert_eq!(full_widths.len(), 5);
}

#[test]
fn table_row_compact_full_uses_placeholder_excerpt_and_unknown_timestamp() {
    let _row = table_row(
        &conversation("thread-1", "/tmp/api", "alpha", "", 0, 0),
        ColumnPreset::Full,
        RowMode::Compact,
        8,
    );
}

#[test]
fn table_row_comfortable_full_keeps_second_excerpt_line_when_needed() {
    let _row = table_row(
        &conversation("thread-1", "/tmp/api", "alpha", &"x".repeat(40), 30, 10),
        ColumnPreset::Full,
        RowMode::Comfortable,
        10,
    );
}

#[test]
fn excerpt_lines_use_mode_specific_visible_lengths() {
    let compact = excerpt_lines(&"x".repeat(40), 10, RowMode::Compact);
    let comfortable = excerpt_lines(&"x".repeat(40), 10, RowMode::Comfortable);

    assert_eq!(compact.len(), 1);
    assert_eq!(compact[0].chars().count(), 10);
    assert!(compact[0].ends_with('…'));
    assert_eq!(comfortable.len(), 2);
    assert_eq!(comfortable[0].chars().count(), 10);
    assert_eq!(comfortable[1].chars().count(), 10);
    assert!(comfortable[1].ends_with('…'));
}

#[test]
fn excerpt_lines_keep_full_second_line_when_content_fits_comfortable_budget() {
    let lines = excerpt_lines("abcdefghijklmno", 10, RowMode::Comfortable);

    assert_eq!(lines, vec!["abcdefghij".to_string(), "klmno".to_string()]);
}

#[test]
fn excerpt_lines_return_empty_lines_when_comfortable_width_is_zero() {
    let lines = excerpt_lines("alpha", 0, RowMode::Comfortable);

    assert_eq!(lines, vec![String::new(), String::new()]);
}

#[test]
fn render_excerpt_uses_more_space_on_wider_terminals() {
    let app = App::new(
        vec![conversation(
            "thread-1",
            "/tmp/api",
            "alpha",
            &"x".repeat(200),
            30,
            10,
        )],
        "/tmp/api".to_string(),
    );

    let narrow = render_to_string_with_size(&app, 90, 20);
    let wide = render_to_string_with_size(&app, 160, 20);

    assert!(wide.matches('x').count() > narrow.matches('x').count());
}

#[test]
fn run_picker_app_resumes_selected_conversation() {
    let resumed = Arc::new(Mutex::new(None::<(String, String, bool)>));
    let resumed_capture = Arc::clone(&resumed);

    let result = run_picker_app(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
        "codex-bin",
        true,
        |_| {
            Ok(conversation(
                "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
            ))
        },
        move |bin, id, dry_run| {
            resumed_capture
                .lock()
                .unwrap_or_else(|_| unreachable!("resume mutex should lock"))
                .replace((bin.to_string(), id.to_string(), dry_run));
            Ok(())
        },
    );

    assert!(result.is_ok());
    assert_eq!(
        resumed
            .lock()
            .unwrap_or_else(|_| unreachable!("resume mutex should lock"))
            .clone(),
        Some(("codex-bin".to_string(), "thread-1".to_string(), true))
    );
}

#[test]
fn run_picker_app_rejects_empty_conversation_lists() {
    let result = run_picker_app(
        Vec::new(),
        "/tmp/api".to_string(),
        "codex-bin",
        false,
        |_| unreachable!("selection should not run"),
        |_, _, _| unreachable!("resume should not run"),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("empty conversation lists should fail"));

    assert!(error.to_string().contains("no Codex conversations found"));
}

#[test]
fn run_picker_app_propagates_selection_errors() {
    let result = run_picker_app(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
        "codex-bin",
        false,
        |_| Err(anyhow::anyhow!("selection failed")),
        |_, _, _| Ok(()),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("selection errors should propagate"));

    assert!(error.to_string().contains("selection failed"));
}

#[test]
fn run_with_rejects_empty_conversation_lists() {
    let result = run_with(
        RunConfig {
            db_path: None,
            session_index_path: None,
            codex_bin: "codex-bin".to_string(),
            dry_run: false,
            include_subagents: false,
        },
        |_db_path, _session_index_path| Ok(Vec::new()),
        || Ok(PathBuf::from("/tmp/api")),
        |_| unreachable!("selection should not run"),
        |_, _, _| unreachable!("resume should not run"),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("empty conversation lists should fail"));

    assert!(error.to_string().contains("no Codex conversations found"));
}

#[test]
fn run_with_propagates_current_directory_failures() {
    let result = run_with(
        RunConfig {
            db_path: None,
            session_index_path: None,
            codex_bin: "codex-bin".to_string(),
            dry_run: false,
            include_subagents: false,
        },
        |_db_path, _session_index_path| {
            Ok(vec![conversation(
                "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
            )])
        },
        || Err(io::Error::other("cwd failed")),
        |_| unreachable!("selection should not run"),
        |_, _, _| unreachable!("resume should not run"),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("current directory failures should propagate"));

    assert!(
        error
            .to_string()
            .contains("failed to determine current directory")
    );
}

#[test]
fn run_with_selects_and_resumes_conversation() {
    let resumed = Arc::new(Mutex::new(None::<(String, String, bool)>));
    let resumed_capture = Arc::clone(&resumed);

    let result = run_with(
        RunConfig {
            db_path: Some(PathBuf::from("/tmp/custom.sqlite")),
            session_index_path: Some(PathBuf::from("/tmp/session_index.jsonl")),
            codex_bin: "codex-bin".to_string(),
            dry_run: true,
            include_subagents: false,
        },
        |db_path, session_index_path| {
            assert_eq!(db_path, Path::new("/tmp/custom.sqlite"));
            assert_eq!(session_index_path, Path::new("/tmp/session_index.jsonl"));
            Ok(vec![conversation(
                "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
            )])
        },
        || Ok(PathBuf::from("/tmp/api")),
        |app| {
            assert_eq!(app.visible_ids(), vec!["thread-1".to_string()]);
            Ok(conversation(
                "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
            ))
        },
        move |bin, id, dry_run| {
            resumed_capture
                .lock()
                .unwrap_or_else(|_| unreachable!("resume mutex should lock"))
                .replace((bin.to_string(), id.to_string(), dry_run));
            Ok(())
        },
    );

    assert!(result.is_ok());
    assert_eq!(
        resumed
            .lock()
            .unwrap_or_else(|_| unreachable!("resume mutex should lock"))
            .clone(),
        Some(("codex-bin".to_string(), "thread-1".to_string(), true))
    );
}

#[test]
fn run_with_can_include_subagents_from_real_state() {
    let temp_dir = TestDir::new("run-with-subagents");
    let db_path = temp_dir.path().join("state.sqlite");
    seed_threads_db_with_subagent(&db_path);

    let resumed = Arc::new(Mutex::new(None::<(String, String, bool)>));
    let resumed_capture = Arc::clone(&resumed);

    let result = run_with(
        RunConfig {
            db_path: Some(db_path),
            session_index_path: Some(temp_dir.path().join("missing.jsonl")),
            codex_bin: "codex-bin".to_string(),
            dry_run: true,
            include_subagents: true,
        },
        |db_path, session_index_path| {
            load_conversations_with_options(db_path, session_index_path, true)
        },
        || Ok(PathBuf::from("/tmp/api")),
        |app| {
            assert_eq!(
                app.visible_ids(),
                vec![
                    "thread-parent".to_string(),
                    "thread-child".to_string(),
                    "thread-plain".to_string()
                ]
            );

            Ok(conversation(
                "thread-child",
                "/tmp/api",
                "child",
                "child preview",
                20,
                10,
            ))
        },
        move |bin, id, dry_run| {
            resumed_capture
                .lock()
                .unwrap_or_else(|_| unreachable!("resume mutex should lock"))
                .replace((bin.to_string(), id.to_string(), dry_run));
            Ok(())
        },
    );

    assert!(result.is_ok());
    assert_eq!(
        resumed
            .lock()
            .unwrap_or_else(|_| unreachable!("resume mutex should lock"))
            .clone(),
        Some(("codex-bin".to_string(), "thread-child".to_string(), true))
    );
}

#[test]
fn run_with_propagates_include_subagents_loader_errors() {
    let temp_dir = TestDir::new("run-with-subagents-error");

    let error = run_with(
        RunConfig {
            db_path: Some(temp_dir.path().to_path_buf()),
            session_index_path: Some(temp_dir.path().join("missing.jsonl")),
            codex_bin: "codex-bin".to_string(),
            dry_run: true,
            include_subagents: true,
        },
        |db_path, session_index_path| {
            load_conversations_with_options(db_path, session_index_path, true)
        },
        || Ok(PathBuf::from("/tmp/api")),
        |_app| unreachable!("loader errors should stop before selection"),
        |_bin, _id, _dry_run| unreachable!("loader errors should stop before resume"),
    )
    .err()
    .unwrap_or_else(|| unreachable!("directory include-subagents database should fail"));

    assert!(error.to_string().contains("failed to open"));
}

#[test]
fn run_with_propagates_load_failures() {
    let result = run_with(
        RunConfig {
            db_path: None,
            session_index_path: None,
            codex_bin: "codex-bin".to_string(),
            dry_run: false,
            include_subagents: false,
        },
        |_db_path, _session_index_path| Err(anyhow::anyhow!("load failed")),
        || Ok(PathBuf::from("/tmp/api")),
        |_| unreachable!("selection should not run"),
        |_, _, _| unreachable!("resume should not run"),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("load failures should propagate"));

    assert!(error.to_string().contains("load failed"));
}

#[test]
fn run_with_propagates_selection_failures() {
    let result = run_with(
        RunConfig {
            db_path: None,
            session_index_path: None,
            codex_bin: "codex-bin".to_string(),
            dry_run: false,
            include_subagents: false,
        },
        |_db_path, _session_index_path| {
            Ok(vec![conversation(
                "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
            )])
        },
        || Ok(PathBuf::from("/tmp/api")),
        |_app| Err(anyhow::anyhow!("selection failed")),
        |_, _, _| unreachable!("resume should not run"),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("selection failures should propagate"));

    assert!(error.to_string().contains("selection failed"));
}

#[test]
fn select_conversation_with_session_returns_selected_conversation() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let init_events = Arc::clone(&events);
    let run_events = Arc::clone(&events);
    let restore_events = Arc::clone(&events);

    let selected = select_conversation_with_session(
        &mut app,
        move || {
            init_events
                .lock()
                .unwrap_or_else(|_| unreachable!("events should lock"))
                .push("init");
            Ok(String::from("terminal"))
        },
        move |terminal, app| {
            run_events
                .lock()
                .unwrap_or_else(|_| unreachable!("events should lock"))
                .push("run");
            terminal.push_str("-used");
            app.selected_conversation()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("selection missing"))
        },
        move |terminal| {
            restore_events
                .lock()
                .unwrap_or_else(|_| unreachable!("events should lock"))
                .push("restore");
            assert_eq!(terminal, "terminal-used");
            Ok(())
        },
    )
    .unwrap_or_else(|_| unreachable!("selection should succeed"));

    assert_eq!(selected.id(), "thread-1");
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(|_| unreachable!("events should lock"))
            .clone(),
        vec!["init", "run", "restore"]
    );
}

#[test]
fn select_conversation_with_session_propagates_restore_errors() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );

    let result = select_conversation_with_session(
        &mut app,
        || Ok(String::from("terminal")),
        |_terminal, app| {
            app.selected_conversation()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("selection missing"))
        },
        |_terminal| Err(anyhow::anyhow!("restore failed")),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("restore errors should propagate"));

    assert!(error.to_string().contains("restore failed"));
}

#[test]
fn select_conversation_supports_fake_terminal_mode() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );

    let selected =
        select_conversation(&mut app).unwrap_or_else(|_| unreachable!("selection should succeed"));

    assert_eq!(selected.id(), "thread-1");
}

#[test]
fn run_default_supports_fake_terminal_mode() {
    let temp_dir = TestDir::new("run-default");
    let db_path = temp_dir.path().join("state.sqlite");
    let codex_path = temp_dir.path().join("fake-codex.sh");
    let log_path = temp_dir.path().join("resume.log");

    seed_threads_db(&db_path);
    write_executable(
        &codex_path,
        &format!(
            "#!/bin/sh\nprintf '%s %s\\n' \"$1\" \"$2\" > '{}'\n",
            log_path.display()
        ),
    );

    let result = run_default(RunConfig {
        db_path: Some(db_path),
        session_index_path: Some(temp_dir.path().join("missing.jsonl")),
        codex_bin: codex_path.display().to_string(),
        dry_run: false,
        include_subagents: false,
    });
    assert!(result.is_ok());

    let log = fs::read_to_string(&log_path)
        .unwrap_or_else(|_| unreachable!("resume log should be written"));
    assert_eq!(log.trim(), "resume thread-1");
}

#[test]
fn run_picker_loop_returns_selected_conversation() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );
    let events = Arc::new(Mutex::new(vec![Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    ))]));
    let reads = Arc::clone(&events);
    let draw_count = Arc::new(Mutex::new(0_usize));
    let draw_counter = Arc::clone(&draw_count);

    let result = run_picker_loop(
        &mut app,
        move |_| {
            let mut counter = draw_counter
                .lock()
                .unwrap_or_else(|_| unreachable!("draw counter should lock"));
            *counter += 1;
            Ok(())
        },
        || Ok(true),
        move || {
            let mut events = reads
                .lock()
                .unwrap_or_else(|_| unreachable!("event queue should lock"));
            Ok(events.remove(0))
        },
    );

    let conversation = result.unwrap_or_else(|_| unreachable!("picker loop should select"));
    assert_eq!(conversation.id, "thread-1");
    assert_eq!(
        *draw_count
            .lock()
            .unwrap_or_else(|_| unreachable!("draw counter should lock")),
        1
    );
}

#[test]
fn run_picker_loop_skips_when_poll_reports_no_event() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );
    let poll_results = Arc::new(Mutex::new(vec![false, true]));
    let polls = Arc::clone(&poll_results);
    let draw_count = Arc::new(Mutex::new(0_usize));
    let draw_counter = Arc::clone(&draw_count);

    let result = run_picker_loop(
        &mut app,
        move |_| {
            let mut counter = draw_counter
                .lock()
                .unwrap_or_else(|_| unreachable!("draw counter should lock"));
            *counter += 1;
            Ok(())
        },
        move || {
            let mut polls = polls
                .lock()
                .unwrap_or_else(|_| unreachable!("poll queue should lock"));
            Ok(polls.remove(0))
        },
        || {
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
        },
    );

    let conversation = result.unwrap_or_else(|_| unreachable!("picker loop should select"));
    assert_eq!(conversation.id, "thread-1");
    assert_eq!(
        *draw_count
            .lock()
            .unwrap_or_else(|_| unreachable!("draw counter should lock")),
        2
    );
}

#[test]
fn run_picker_loop_continues_after_key_without_selection() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );
    let events = Arc::new(Mutex::new(vec![
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ]));
    let reads = Arc::clone(&events);

    let result = run_picker_loop(
        &mut app,
        |_| Ok(()),
        || Ok(true),
        move || {
            let mut events = reads
                .lock()
                .unwrap_or_else(|_| unreachable!("event queue should lock"));
            Ok(events.remove(0))
        },
    );

    let conversation = result.unwrap_or_else(|_| unreachable!("picker loop should select"));
    assert_eq!(conversation.id, "thread-1");
    assert_eq!(app.query, "");
}

#[test]
fn run_picker_loop_propagates_handle_key_event_errors() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );

    let result = run_picker_loop(
        &mut app,
        |_| Ok(()),
        || Ok(true),
        || Ok(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("escape should propagate the cancel signal"));
    assert!(error.to_string().contains("selection cancelled"));
}

#[test]
fn run_picker_loop_skips_non_key_events() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );
    let events = Arc::new(Mutex::new(vec![
        Event::Resize(120, 40),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ]));
    let reads = Arc::clone(&events);

    let result = run_picker_loop(
        &mut app,
        |_| Ok(()),
        || Ok(true),
        move || {
            let mut events = reads
                .lock()
                .unwrap_or_else(|_| unreachable!("event queue should lock"));
            Ok(events.remove(0))
        },
    );

    let conversation = result.unwrap_or_else(|_| unreachable!("picker loop should select"));
    assert_eq!(conversation.id, "thread-1");
}

#[test]
fn run_picker_loop_propagates_draw_errors() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );

    let result = run_picker_loop(
        &mut app,
        |_| Err(anyhow::anyhow!("draw failed")),
        || Ok(true),
        || {
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
        },
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("draw errors should propagate"));

    assert!(error.to_string().contains("draw failed"));
}

#[test]
fn run_picker_loop_propagates_poll_errors() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );

    let result = run_picker_loop(
        &mut app,
        |_| Ok(()),
        || Err(anyhow::anyhow!("poll failed")),
        || {
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
        },
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("poll errors should propagate"));

    assert!(error.to_string().contains("poll failed"));
}

#[test]
fn run_picker_loop_propagates_read_errors() {
    let mut app = App::new(
        vec![conversation(
            "thread-1", "/tmp/api", "alpha", "preview", 30, 10,
        )],
        "/tmp/api".to_string(),
    );

    let result = run_picker_loop(
        &mut app,
        |_| Ok(()),
        || Ok(true),
        || Err(anyhow::anyhow!("read failed")),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("read errors should propagate"));

    assert!(error.to_string().contains("read failed"));
}

#[test]
fn with_terminal_session_runs_and_restores_in_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let init_events = Arc::clone(&events);
    let run_events = Arc::clone(&events);
    let restore_events = Arc::clone(&events);

    let result = with_terminal_session(
        move || {
            init_events
                .lock()
                .unwrap_or_else(|_| unreachable!("events should lock"))
                .push("init");
            Ok(String::from("terminal"))
        },
        move |terminal| {
            run_events
                .lock()
                .unwrap_or_else(|_| unreachable!("events should lock"))
                .push("run");
            terminal.push_str("-used");
            Ok(terminal.clone())
        },
        move |terminal| {
            restore_events
                .lock()
                .unwrap_or_else(|_| unreachable!("events should lock"))
                .push("restore");
            assert_eq!(terminal, "terminal-used");
            Ok(())
        },
    );

    let value = result.unwrap_or_else(|_| unreachable!("session should succeed"));
    assert_eq!(value, "terminal-used".to_string());
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(|_| unreachable!("events should lock"))
            .clone(),
        vec!["init", "run", "restore"]
    );
}

#[test]
fn with_terminal_session_prefers_restore_error_after_run_failure() {
    let result: anyhow::Result<String> = with_terminal_session(
        || Ok(String::from("terminal")),
        |_| Err(anyhow::anyhow!("run failed")),
        |_| Err(anyhow::anyhow!("restore failed")),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("restore errors should propagate"));

    assert!(error.to_string().contains("restore failed"));
}

#[test]
fn with_terminal_session_propagates_init_failure() {
    let result: anyhow::Result<String> = with_terminal_session(
        || Err(anyhow::anyhow!("init failed")),
        |_terminal: &mut String| Ok(String::new()),
        |_terminal: &mut String| Ok(()),
    );

    let error = result
        .err()
        .unwrap_or_else(|| unreachable!("init errors should propagate"));

    assert!(error.to_string().contains("init failed"));
}

#[test]
fn init_terminal_session_propagates_errors_and_builds_terminal() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let enable_order = Arc::clone(&order);
    let backend_order = Arc::clone(&order);
    let build_order = Arc::clone(&order);

    let value = init_terminal_session(
        move || {
            enable_order
                .lock()
                .unwrap_or_else(|_| unreachable!("order should lock"))
                .push("enable");
            Ok(())
        },
        move || {
            backend_order
                .lock()
                .unwrap_or_else(|_| unreachable!("order should lock"))
                .push("backend");
            Ok(String::from("backend"))
        },
        move |backend| {
            build_order
                .lock()
                .unwrap_or_else(|_| unreachable!("order should lock"))
                .push("build");
            Ok(format!("terminal:{backend}"))
        },
    )
    .unwrap_or_else(|_| unreachable!("terminal session should build"));

    assert_eq!(value, "terminal:backend".to_string());
    assert_eq!(
        order
            .lock()
            .unwrap_or_else(|_| unreachable!("order should lock"))
            .clone(),
        vec!["enable", "backend", "build"]
    );

    let error = init_terminal_session(
        || Err(anyhow::anyhow!("enable failed")),
        || Ok(String::from("backend")),
        |_backend| Ok(String::from("terminal")),
    )
    .err()
    .unwrap_or_else(|| unreachable!("enable errors should propagate"));
    assert!(error.to_string().contains("enable failed"));

    let error = init_terminal_session(
        || Ok(()),
        || Err(anyhow::anyhow!("backend failed")),
        |_backend: String| Ok(String::from("terminal")),
    )
    .err()
    .unwrap_or_else(|| unreachable!("backend errors should propagate"));
    assert!(error.to_string().contains("backend failed"));

    let error = init_terminal_session(
        || Ok(()),
        || Ok(String::from("backend")),
        |_backend: String| -> anyhow::Result<String> { Err(anyhow::anyhow!("build failed")) },
    )
    .err()
    .unwrap_or_else(|| unreachable!("build errors should propagate"));
    assert!(error.to_string().contains("build failed"));
}

#[test]
fn restore_terminal_session_propagates_error_in_sequence() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let disable_order = Arc::clone(&order);
    let leave_order = Arc::clone(&order);
    let cursor_order = Arc::clone(&order);

    let result = restore_terminal_session(
        &mut String::from("terminal"),
        move || {
            disable_order
                .lock()
                .unwrap_or_else(|_| unreachable!("order should lock"))
                .push("disable");
            Ok(())
        },
        move |_terminal| {
            leave_order
                .lock()
                .unwrap_or_else(|_| unreachable!("order should lock"))
                .push("leave");
            Ok(())
        },
        move |_terminal| {
            cursor_order
                .lock()
                .unwrap_or_else(|_| unreachable!("order should lock"))
                .push("cursor");
            Ok(())
        },
    );
    assert!(result.is_ok());
    assert_eq!(
        order
            .lock()
            .unwrap_or_else(|_| unreachable!("order should lock"))
            .clone(),
        vec!["disable", "leave", "cursor"]
    );

    let error = restore_terminal_session(
        &mut String::from("terminal"),
        || Ok(()),
        |_terminal| Err(anyhow::anyhow!("leave failed")),
        |_terminal| Ok(()),
    )
    .err()
    .unwrap_or_else(|| unreachable!("leave errors should propagate"));

    assert!(error.to_string().contains("leave failed"));

    let error = restore_terminal_session(
        &mut String::from("terminal"),
        || Err(anyhow::anyhow!("disable failed")),
        |_terminal| Ok(()),
        |_terminal| Ok(()),
    )
    .err()
    .unwrap_or_else(|| unreachable!("disable errors should propagate"));
    assert!(error.to_string().contains("disable failed"));

    let error = restore_terminal_session(
        &mut String::from("terminal"),
        || Ok(()),
        |_terminal| Ok(()),
        |_terminal| Err(anyhow::anyhow!("cursor failed")),
    )
    .err()
    .unwrap_or_else(|| unreachable!("cursor errors should propagate"));

    assert!(error.to_string().contains("cursor failed"));
}

#[test]
fn resume_conversation_executes_subprocess_and_reports_failures() {
    let temp_dir = TestDir::new("resume-subprocess");
    let success_script = temp_dir.path().join("resume-success.sh");
    let failure_script = temp_dir.path().join("resume-failure.sh");

    write_executable(
        &success_script,
        "#!/bin/sh\n[ \"$1\" = \"resume\" ] && [ \"$2\" = \"thread-1\" ]\n",
    );
    write_executable(&failure_script, "#!/bin/sh\nexit 17\n");

    let success_result =
        resume_conversation(success_script.to_str().unwrap_or(""), "thread-1", false);
    assert!(success_result.is_ok());

    let error = resume_conversation(failure_script.to_str().unwrap_or(""), "thread-1", false)
        .err()
        .unwrap_or_else(|| unreachable!("non-zero exit status should fail"));
    assert!(error.to_string().contains("exited with"));
}

#[test]
fn write_dry_run_output_writes_and_reports_errors() {
    let mut buffer = Vec::new();

    let result = write_dry_run_output(&mut buffer, "codex", "thread-1");
    assert!(result.is_ok());
    assert_eq!(
        String::from_utf8(buffer).unwrap_or_else(|_| unreachable!("utf8 should decode")),
        "codex resume thread-1\n"
    );

    let error = write_dry_run_output(&mut FailingWriter, "codex", "thread-1")
        .err()
        .unwrap_or_else(|| unreachable!("writer failures should propagate"));
    assert!(error.to_string().contains("failed to write dry-run output"));
}

#[test]
fn resume_conversation_supports_dry_run_mode() {
    let result = resume_conversation("codex", "thread-1", true);
    assert!(result.is_ok());
}

#[test]
fn resume_conversation_reports_spawn_failures() {
    let error = resume_conversation("/definitely/missing/codex-bin", "thread-1", false)
        .err()
        .unwrap_or_else(|| unreachable!("spawn failures should propagate"));

    assert!(error.to_string().contains("failed to execute"));
}

fn render_to_string(app: &App) -> String {
    render_to_string_with_size(app, 140, 20)
}

fn render_to_string_with_size(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal =
        Terminal::new(backend).unwrap_or_else(|_| unreachable!("test terminal should initialize"));

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
    let connection =
        Connection::open(db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));

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

fn seed_threads_db_with_subagent(db_path: &Path) {
    let connection =
        Connection::open(db_path).unwrap_or_else(|_| unreachable!("sqlite database should open"));

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
                archived integer not null,
                source text,
                thread_source text
            );
            create table thread_spawn_edges (
                parent_thread_id text not null,
                child_thread_id text not null,
                status text not null
            );",
    );
    assert!(schema_result.is_ok());

    let parent_insert = connection.execute(
        "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived, source, thread_source
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        (
            "thread-parent",
            "/tmp/api",
            "parent",
            "parent preview",
            "parent message",
            3_000_i64,
            2_500_i64,
            2_000_i64,
            2_i64,
            0_i64,
            "cli",
            Option::<String>::None,
        ),
    );
    assert!(parent_insert.is_ok());

    let child_insert = connection.execute(
        "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived, source, thread_source
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        (
            "thread-child",
            "/tmp/api",
            "child",
            "child preview",
            "child message",
            2_000_i64,
            1_500_i64,
            1_000_i64,
            1_i64,
            0_i64,
            r#"{"subagent":{"thread_spawn":{"parent_thread_id":"thread-parent","depth":1}}}"#,
            "subagent",
        ),
    );
    assert!(child_insert.is_ok());

    let plain_insert = connection.execute(
        "insert into threads (
                id, cwd, title, preview, first_user_message, recency_at_ms,
                updated_at_ms, created_at_ms, created_at, archived, source, thread_source
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        (
            "thread-plain",
            "/tmp/api",
            "plain",
            "plain preview",
            "plain message",
            1_000_i64,
            900_i64,
            800_i64,
            1_i64,
            0_i64,
            "cli",
            Option::<String>::None,
        ),
    );
    assert!(plain_insert.is_ok());

    let edge_insert = connection.execute(
        "insert into thread_spawn_edges (parent_thread_id, child_thread_id, status)
            values (?1, ?2, ?3)",
        ("thread-parent", "thread-child", "closed"),
    );
    assert!(edge_insert.is_ok());
}

fn write_file(path: &Path, contents: &str) {
    let write_result = fs::write(path, contents);
    assert!(write_result.is_ok());
}

fn write_executable(path: &Path, contents: &str) {
    write_file(path, contents);

    let metadata = fs::metadata(path)
        .unwrap_or_else(|_| unreachable!("executable metadata should be readable"));
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o755);

    let set_permissions_result = fs::set_permissions(path, permissions);
    assert!(set_permissions_result.is_ok());
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0_u128, |duration| duration.as_nanos());
        let path = env::temp_dir().join(format!(
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
        is_subagent: false,
    }
}
