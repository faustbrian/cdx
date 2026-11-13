//! Loading of local Claude Code sessions from `~/.claude/projects`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::DateTime;
use serde::Deserialize;

use crate::coverage_excluded::{
    deduplicate_conversations, display_title, preview_text, read_session_files,
};
use crate::{Agent, Conversation, render_cwd};

/// Session files can reach hundreds of megabytes, so only their head is read.
/// The working directory, custom title, and first prompt are written within
/// the first few dozen lines of every session observed so far.
const SESSION_HEAD_BYTES: u64 = 512 * 1024;
/// Renaming a session appends a new `custom-title` record, and Claude Code
/// re-emits the current title every few turns; across all local sessions the
/// last one sat at most 34 KB from the end of its file.
const SESSION_TAIL_BYTES: u64 = 128 * 1024;
const TITLE_MARKERS: [&[u8]; 2] = [b"\"custom-title\"", b"\"ai-title\""];

#[derive(Debug, Deserialize)]
struct SessionLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    entrypoint: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    /// Marks records Claude Code injects, such as a skill's instructions.
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    message: Option<SessionMessage>,
}

#[derive(Debug, Deserialize)]
struct SessionMessage {
    content: Option<MessageContent>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Parts(Vec<MessagePart>),
}

#[derive(Debug, Deserialize)]
struct MessagePart {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct SessionHead {
    pub(super) cwd: Option<String>,
    pub(super) custom_title: Option<String>,
    pub(super) ai_title: Option<String>,
    pub(super) first_prompt: Option<String>,
    pub(super) created_at_ms: Option<i64>,
    pub(super) is_non_interactive: bool,
}

impl SessionHead {
    /// Absorbs one raw JSONL record and reports whether every field is known.
    ///
    /// The byte cap can cut the last line mid-record; unparseable lines and
    /// record types this picker does not use are skipped.
    pub(super) fn absorb_line(&mut self, line: &[u8]) -> bool {
        if let Ok(record) = serde_json::from_slice::<SessionLine>(line) {
            self.absorb(record);
        }

        self.cwd.is_some()
            && self.custom_title.is_some()
            && self.first_prompt.is_some()
            && self.created_at_ms.is_some()
    }

    /// Absorbs one raw JSONL record from the end of the file, where the most
    /// recent `custom-title` and `ai-title` records hold the current names.
    pub(super) fn absorb_tail_line(&mut self, line: &[u8]) {
        let mentions_title = TITLE_MARKERS
            .iter()
            .any(|marker| line.windows(marker.len()).any(|window| window == *marker));
        if !mentions_title {
            return;
        }

        if let Ok(record) = serde_json::from_slice::<SessionLine>(line) {
            if record.custom_title.is_some() {
                self.custom_title = record.custom_title;
            }
            if record.ai_title.is_some() {
                self.ai_title = record.ai_title;
            }
        }
    }

    /// The user's name for the session, else the one Claude Code generated.
    pub(super) fn title(&self) -> Option<&str> {
        self.custom_title.as_deref().or(self.ai_title.as_deref())
    }

    fn absorb(&mut self, line: SessionLine) {
        if self.cwd.is_none() {
            self.cwd = line.cwd.filter(|cwd| !cwd.trim().is_empty());
        }
        if self.created_at_ms.is_none() {
            self.created_at_ms = line.timestamp.as_deref().and_then(parse_timestamp_ms);
        }
        if line
            .entrypoint
            .as_deref()
            .is_some_and(|entrypoint| entrypoint.starts_with("sdk"))
        {
            self.is_non_interactive = true;
        }
        if self.custom_title.is_none() {
            self.custom_title = line.custom_title;
        }
        if line.ai_title.is_some() {
            self.ai_title = line.ai_title;
        }
        if self.first_prompt.is_none()
            && line.kind.as_deref() == Some("user")
            && line.is_sidechain != Some(true)
            && line.is_meta != Some(true)
        {
            self.first_prompt = line
                .message
                .and_then(|message| message.content)
                .and_then(MessageContent::into_prompt);
        }
    }
}

impl MessageContent {
    fn into_prompt(self) -> Option<String> {
        let text = match self {
            Self::Text(text) => text,
            Self::Parts(parts) => parts
                .into_iter()
                .find(|part| part.kind.as_deref() == Some("text"))
                .and_then(|part| part.text)?,
        };
        let trimmed = text.trim();

        // Hook output and command stdout are stored as user messages wrapped
        // in XML-like tags; they are not the user's prompt. A slash command is
        // stored the same way, so it is rebuilt from its name and arguments.
        if trimmed.starts_with('<') {
            let command = [
                tag_text(trimmed, "command-name"),
                tag_text(trimmed, "command-args"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
            return Some(command).filter(|command| !command.is_empty());
        }

        Some(trimmed.to_string()).filter(|prompt| !prompt.is_empty())
    }
}

fn tag_text<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{tag}>"))? + start;

    Some(text[start..end].trim()).filter(|value| !value.is_empty())
}

fn parse_timestamp_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

/// Returns the default Claude Code projects directory under the current home directory.
#[must_use]
pub fn default_claude_projects_path() -> PathBuf {
    default_claude_projects_path_from_home(crate::home_dir())
}

pub(super) fn default_claude_projects_path_from_home(home_dir: Option<PathBuf>) -> PathBuf {
    home_dir
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".claude")
        .join("projects")
}

/// Loads Claude Code sessions stored as `<projects>/<project>/<session>.jsonl`.
///
/// A missing projects directory yields no conversations, so machines without
/// Claude Code still show their Codex history.
///
/// # Errors
///
/// Returns an error when the projects directory or a session file exists but
/// cannot be read.
pub fn load_claude_conversations(
    projects_path: &Path,
    include_subagents: bool,
) -> Result<Vec<Conversation>> {
    let conversations = read_session_files(projects_path, SESSION_HEAD_BYTES, SESSION_TAIL_BYTES)?
        .into_iter()
        .filter_map(SessionFile::into_conversation)
        .filter(|conversation| include_subagents || !conversation.is_subagent)
        .collect();

    Ok(deduplicate_conversations(conversations))
}

/// The head of one `<session>.jsonl` file and its modification time.
#[derive(Debug)]
pub(super) struct SessionFile {
    pub(super) id: String,
    pub(super) head: SessionHead,
    pub(super) updated_at_ms: i64,
}

impl SessionFile {
    fn into_conversation(self) -> Option<Conversation> {
        let Self {
            id,
            head,
            updated_at_ms,
        } = self;
        let cwd = head.cwd.clone()?;

        Some(Conversation {
            display_title: display_title(&id, head.title(), None),
            preview: preview_text(head.first_prompt.as_deref(), None),
            id,
            rendered_cwd: render_cwd(&cwd),
            cwd,
            updated_at_ms,
            created_at_ms: head
                .created_at_ms
                .unwrap_or(updated_at_ms)
                .min(updated_at_ms),
            is_subagent: head.is_non_interactive,
            agent: Agent::Claude,
        })
    }
}
