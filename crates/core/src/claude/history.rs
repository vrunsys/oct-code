//! A project's existing Claude Code sessions, read from Claude Code's own
//! history files (`<claude home>/projects/<encoded folder>/<session>.jsonl`).
//!
//! Read-only. A file counts only if it is named like a session id and its
//! recorded `cwd` is this project: the folder encoding is lossy, so two
//! folders can share a directory. Subagent (`isSidechain`) and meta lines,
//! and command wrappers such as `<command-name>`, are not conversation.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{AgentEvent, is_valid_session_id, parse_line};

/// History files above this size are skipped rather than read on import.
const MAX_SESSION_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum HistoryEntry {
    User(String),
    Agent(AgentEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistorySession {
    pub session_id: String,
    /// Claude Code's own title for the session, when it made one.
    pub title: Option<String>,
    pub entries: Vec<HistoryEntry>,
    /// When the history file last changed, in Unix milliseconds.
    pub modified_ms: u64,
}

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_home() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

/// The directory name Claude Code uses for a project: every character that
/// is not an ASCII letter or digit becomes `-`.
pub fn encode_project_dir(project: &Path) -> String {
    project
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Every readable session Claude Code recorded for `project`.
pub fn sessions_for_project(claude_home: &Path, project: &Path) -> Vec<HistorySession> {
    HistoryWatcher::new(claude_home, project).poll()
}

/// Follows a project's history while Claude Code writes to it from
/// elsewhere, such as the terminal or the VS Code extension. Each
/// [`poll`](Self::poll) re-reads only the files that are new or changed.
#[derive(Debug)]
pub struct HistoryWatcher {
    dir: PathBuf,
    project: PathBuf,
    /// Each session file's size and modification time at the last poll.
    seen: HashMap<String, FileStamp>,
}

type FileStamp = (u64, Option<SystemTime>);

impl HistoryWatcher {
    pub fn new(claude_home: &Path, project: &Path) -> Self {
        Self {
            dir: claude_home
                .join("projects")
                .join(encode_project_dir(project)),
            project: project.to_path_buf(),
            seen: HashMap::new(),
        }
    }

    /// The readable sessions whose files changed since the last poll; the
    /// first poll returns every one.
    pub fn poll(&mut self) -> Vec<HistorySession> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            self.seen.clear();
            return Vec::new();
        };
        let mut seen = HashMap::new();
        let mut changed = Vec::new();
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            let Some(session_id) = session_id_of(&path) else {
                continue;
            };
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            let stamp = (metadata.len(), metadata.modified().ok());
            // A file that was unreadable or someone else's stays skipped
            // until it changes, so it is not read again on every poll.
            if self.seen.get(&session_id) != Some(&stamp)
                && let Some(session) =
                    read_session(&path, session_id.clone(), &self.project, &metadata)
            {
                changed.push(session);
            }
            seen.insert(session_id, stamp);
        }
        self.seen = seen;
        changed
    }
}

/// The session id a history file is named after, if it is one.
fn session_id_of(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|_| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter(|stem| is_valid_session_id(stem))
        .map(str::to_owned)
}

fn read_session(
    path: &Path,
    session_id: String,
    project: &Path,
    metadata: &fs::Metadata,
) -> Option<HistorySession> {
    if metadata.len() > MAX_SESSION_FILE_BYTES {
        log::warn!("skipping large session history {}", path.display());
        return None;
    }
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });

    let mut session = HistorySession {
        session_id,
        title: None,
        entries: Vec::new(),
        modified_ms,
    };
    let mut belongs: Option<bool> = None;
    for line in BufReader::new(fs::File::open(path).ok()?).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("ai-title") => {
                session.title = value
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some(kind @ ("user" | "assistant")) => {
                if is_true(&value, "isSidechain") || is_true(&value, "isMeta") {
                    continue;
                }
                if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
                    // The first recorded cwd decides where the session ran.
                    let ran_here = *belongs.get_or_insert_with(|| Path::new(cwd) == project);
                    if !ran_here {
                        return None;
                    }
                }
                if kind == "user"
                    && let Some(text) = user_text(&value)
                {
                    session.entries.push(HistoryEntry::User(text));
                }
                // Tool results (user lines) and assistant text and tool calls
                // share the stream-json shapes.
                let events = parse_line(&line).unwrap_or_default();
                session
                    .entries
                    .extend(events.into_iter().map(HistoryEntry::Agent));
            }
            _ => {}
        }
    }
    let has_user_message = session
        .entries
        .iter()
        .any(|entry| matches!(entry, HistoryEntry::User(_)));
    (belongs == Some(true) && has_user_message).then_some(session)
}

/// The text the user typed, or `None` for tool results and command wrappers.
fn user_text(value: &Value) -> Option<String> {
    let text = match value.pointer("/message/content")? {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_owned())
}

fn is_true(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}
