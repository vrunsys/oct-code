//! One conversation with an agent, and the reducer that folds
//! [`AgentEvent`]s into it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::ApprovalRequest;
use crate::claude::AgentEvent;

const UNTITLED: &str = "New thread";
const TITLE_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

impl ThreadId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThreadStatus {
    Idle,
    Running,
    Failed(String),
}

/// Whether Claude asks before using tools in a thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// Each tool use that Claude Code would ask about waits for Allow or Deny.
    #[default]
    Supervised,
    /// Claude uses every tool without asking.
    FullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalState {
    Pending,
    Allowed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolOutcome {
    Ok(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Item {
    User(String),
    Assistant(String),
    /// A summary of Claude's thinking, shown before what it led to.
    Thinking(String),
    /// Claude asked to use a tool and waits for the person to decide.
    Approval {
        request_id: String,
        tool_name: String,
        input: Value,
        state: ApprovalState,
    },
    Tool {
        id: String,
        name: String,
        input: Value,
        outcome: Option<ToolOutcome>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub project: PathBuf,
    pub title: String,
    pub session_id: Option<String>,
    pub items: Vec<Item>,
    pub status: ThreadStatus,
    pub cost_usd: f64,
    /// Creation time in Unix milliseconds; 0 for threads saved before this field existed.
    #[serde(default)]
    pub created_at: u64,
    /// The model id chosen for this thread; `None` uses Claude's default.
    #[serde(default)]
    pub model: Option<String>,
    /// The reasoning effort chosen for this thread; `None` uses the model's default.
    #[serde(default)]
    pub effort: Option<String>,
    /// Threads saved before modes existed load as supervised.
    #[serde(default)]
    pub permission_mode: PermissionMode,
    /// Whether the last item is text or thinking Claude is still writing.
    #[serde(skip)]
    streaming: bool,
}

impl Thread {
    pub fn new(project: PathBuf) -> Self {
        Self {
            id: ThreadId::new(),
            project,
            title: UNTITLED.to_owned(),
            session_id: None,
            items: Vec::new(),
            status: ThreadStatus::Idle,
            cost_usd: 0.0,
            created_at: unix_millis_now(),
            model: None,
            effort: None,
            permission_mode: PermissionMode::default(),
            streaming: false,
        }
    }

    /// Records a message the user sent; the first one becomes the title.
    pub fn push_user(&mut self, text: &str) {
        let is_first = !self.items.iter().any(|item| matches!(item, Item::User(_)));
        if is_first {
            let first_line = text.trim().lines().next().unwrap_or_default();
            if !first_line.is_empty() {
                self.title = first_line.chars().take(TITLE_MAX_CHARS).collect();
            }
        }
        self.items.push(Item::User(text.to_owned()));
        self.status = ThreadStatus::Running;
        self.streaming = false;
    }

    pub fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Init { session_id, .. } => self.session_id = Some(session_id.clone()),
            AgentEvent::AssistantDelta { text } => self.stream(text, Item::Assistant),
            AgentEvent::ThinkingDelta { text } => self.stream(text, Item::Thinking),
            AgentEvent::AssistantText { text } => self.finish(text, Item::Assistant),
            AgentEvent::Thinking { text } => self.finish(text, Item::Thinking),
            AgentEvent::ToolUse { id, name, input } => self.push_finished(Item::Tool {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                outcome: None,
            }),
            AgentEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let tool = self.items.iter_mut().rev().find_map(|item| match item {
                    Item::Tool { id, outcome, .. } if id == tool_use_id => Some(outcome),
                    _ => None,
                });
                if let Some(outcome) = tool {
                    *outcome = Some(if *is_error {
                        ToolOutcome::Error(content.clone())
                    } else {
                        ToolOutcome::Ok(content.clone())
                    });
                }
            }
            AgentEvent::TurnDone {
                is_error,
                result,
                cost_usd,
                ..
            } => {
                self.streaming = false;
                self.cost_usd += cost_usd.unwrap_or(0.0);
                self.status = if *is_error {
                    ThreadStatus::Failed(result.clone())
                } else {
                    ThreadStatus::Idle
                };
            }
            AgentEvent::Unknown { .. } => {}
        }
    }

    /// Shows Claude's request to use a tool, waiting for a decision.
    pub fn push_approval(&mut self, request: &ApprovalRequest) {
        self.push_finished(Item::Approval {
            request_id: request.request_id.clone(),
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            state: ApprovalState::Pending,
        });
    }

    /// Records the decision on a pending request. Returns `false` when there
    /// is no such request or it was already decided.
    pub fn resolve_approval(&mut self, request_id: &str, allowed: bool) -> bool {
        let pending = self.items.iter_mut().rev().find_map(|item| match item {
            Item::Approval {
                request_id: id,
                state: state @ ApprovalState::Pending,
                ..
            } if id == request_id => Some(state),
            _ => None,
        });
        let Some(state) = pending else {
            return false;
        };
        *state = if allowed {
            ApprovalState::Allowed
        } else {
            ApprovalState::Denied
        };
        true
    }

    /// Request ids of the approvals still waiting for a decision.
    pub fn pending_approvals(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|item| match item {
            Item::Approval {
                request_id,
                state: ApprovalState::Pending,
                ..
            } => Some(request_id.as_str()),
            _ => None,
        })
    }

    /// Adds a piece to the open block of the same kind, or opens one.
    fn stream(&mut self, text: &str, kind: fn(String) -> Item) {
        match self.open_block(kind) {
            Some(open) => open.push_str(text),
            None => self.items.push(kind(text.to_owned())),
        }
        self.streaming = true;
    }

    /// The finished block is the truth; it replaces what streamed in.
    fn finish(&mut self, text: &str, kind: fn(String) -> Item) {
        match self.open_block(kind) {
            Some(open) => text.clone_into(open),
            None => self.items.push(kind(text.to_owned())),
        }
        self.streaming = false;
    }

    /// The text of the last item, if it is a `kind` block still streaming.
    fn open_block(&mut self, kind: fn(String) -> Item) -> Option<&mut String> {
        if !self.streaming {
            return None;
        }
        match (self.items.last_mut()?, kind(String::new())) {
            (Item::Assistant(open), Item::Assistant(_))
            | (Item::Thinking(open), Item::Thinking(_)) => Some(open),
            _ => None,
        }
    }

    fn push_finished(&mut self, item: Item) {
        self.items.push(item);
        self.streaming = false;
    }

    /// The agent process ended. Only an exit in the middle of a turn is a failure.
    pub fn on_exit(&mut self, code: Option<i32>, stderr_tail: &str) {
        self.streaming = false;
        if self.status != ThreadStatus::Running {
            return;
        }
        let how = match code {
            Some(code) => format!("code {code}"),
            None => "killed".to_owned(),
        };
        let tail = stderr_tail.trim();
        self.status = ThreadStatus::Failed(if tail.is_empty() {
            format!("Agent exited ({how})")
        } else {
            format!("Agent exited ({how}): {tail}")
        });
    }
}

fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
