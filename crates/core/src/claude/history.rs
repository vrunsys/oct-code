//! A project's existing Claude Code sessions, read from Claude Code's own
//! history files (`<claude home>/projects/<encoded folder>/<session>.jsonl`).
//!
//! Read-only. A file counts only if it is named like a session id and its
//! recorded `cwd` is this project: the folder encoding is lossy, so two
//! folders can share a directory. Subagent (`isSidechain`) and meta lines,
//! and command wrappers such as `<command-name>`, are not conversation.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
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
    /// Whether `entries` hold the whole session, rather than only what was
    /// added since the session was last reported.
    pub from_start: bool,
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
/// elsewhere, such as the terminal or the VS Code extension. Claude Code
/// only appends to a session file, so each [`poll`](Self::poll) reads just
/// the lines added since the last one.
#[derive(Debug)]
pub struct HistoryWatcher {
    dir: PathBuf,
    project: PathBuf,
    files: HashMap<String, FollowedFile>,
}

type FileStamp = (u64, Option<SystemTime>);

#[derive(Debug)]
struct FollowedFile {
    stamp: FileStamp,
    /// Bytes read so far; always the end of a whole line.
    offset: u64,
    reader: SessionReader,
    /// Whether the session was reported, so later reports carry only what
    /// was added.
    reported: bool,
}

impl FollowedFile {
    fn new(stamp: FileStamp) -> Self {
        Self {
            stamp,
            offset: 0,
            reader: SessionReader::default(),
            reported: false,
        }
    }
}

impl HistoryWatcher {
    pub fn new(claude_home: &Path, project: &Path) -> Self {
        Self {
            dir: claude_home
                .join("projects")
                .join(encode_project_dir(project)),
            project: project.to_path_buf(),
            files: HashMap::new(),
        }
    }

    /// The sessions that changed since the last poll. A session's first
    /// report has all its entries ([`HistorySession::from_start`]); later
    /// ones have only the entries added since.
    pub fn poll(&mut self) -> Vec<HistorySession> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            self.files.clear();
            return Vec::new();
        };
        let mut present = Vec::new();
        let mut changed = Vec::new();
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            let Some(session_id) = session_id_of(&path) else {
                continue;
            };
            present.push(session_id.clone());
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            let stamp = (metadata.len(), metadata.modified().ok());
            let file = self
                .files
                .entry(session_id.clone())
                .or_insert_with(|| FollowedFile::new((u64::MAX, None)));
            if file.stamp == stamp {
                continue;
            }
            if metadata.len() < file.offset {
                // Rewritten rather than appended to: start over.
                *file = FollowedFile::new(stamp);
            }
            file.stamp = stamp;
            if let Some(session) = follow(&path, session_id, &self.project, file, &metadata) {
                changed.push(session);
            }
        }
        self.files
            .retain(|session_id, _| present.contains(session_id));
        changed
    }
}

/// Reads what was added to a followed file, and reports it when the
/// session is one of this project's and has something new.
fn follow(
    path: &Path,
    session_id: String,
    project: &Path,
    file: &mut FollowedFile,
    metadata: &fs::Metadata,
) -> Option<HistorySession> {
    if file.offset == 0 && metadata.len() > MAX_SESSION_FILE_BYTES {
        log::warn!("skipping large session history {}", path.display());
        return None;
    }
    if file.reader.belongs == Some(false) {
        return None;
    }
    let added = match read_whole_lines(path, file.offset) {
        Ok(added) => added,
        Err(error) => {
            log::warn!("reading {}: {error}", path.display());
            return None;
        }
    };
    file.offset += added.len() as u64;
    let title_before = file.reader.title.clone();
    for line in String::from_utf8_lossy(&added).lines() {
        file.reader.ingest(line, project);
    }
    let reader = &mut file.reader;
    let is_new = !reader.entries.is_empty() || reader.title != title_before;
    if reader.belongs != Some(true) || !reader.has_user_message || !is_new && file.reported {
        return None;
    }
    let from_start = !file.reported;
    file.reported = true;
    Some(HistorySession {
        session_id,
        title: reader.title.clone(),
        entries: std::mem::take(&mut reader.entries),
        modified_ms: modified_ms(metadata),
        from_start,
    })
}

/// The bytes from `offset` up to the end of the last complete line; a line
/// still being written is left for the next read.
fn read_whole_lines(path: &Path, offset: u64) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let whole = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    bytes.truncate(whole);
    Ok(bytes)
}

fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The session id a history file is named after, if it is one.
fn session_id_of(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|_| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter(|stem| is_valid_session_id(stem))
        .map(str::to_owned)
}

/// Turns a session file's lines into entries, one line at a time.
#[derive(Debug, Default)]
struct SessionReader {
    title: Option<String>,
    /// Whether the session ran in this project; the first recorded `cwd`
    /// decides.
    belongs: Option<bool>,
    has_user_message: bool,
    /// Entries not yet reported.
    entries: Vec<HistoryEntry>,
}

impl SessionReader {
    fn ingest(&mut self, line: &str, project: &Path) {
        if self.belongs == Some(false) {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("ai-title") => {
                self.title = value
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some(kind @ ("user" | "assistant")) => {
                if is_true(&value, "isSidechain") || is_true(&value, "isMeta") {
                    return;
                }
                if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
                    let ran_here = *self
                        .belongs
                        .get_or_insert_with(|| Path::new(cwd) == project);
                    if !ran_here {
                        self.entries.clear();
                        return;
                    }
                }
                if kind == "user"
                    && let Some(text) = user_text(&value)
                {
                    self.has_user_message = true;
                    self.entries.push(HistoryEntry::User(text));
                }
                // Tool results (user lines) and assistant text and tool calls
                // share the stream-json shapes.
                let events = parse_line(line).unwrap_or_default();
                self.entries
                    .extend(events.into_iter().map(HistoryEntry::Agent));
            }
            _ => {}
        }
    }
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
