//! One conversation with an agent, and the reducer that folds
//! [`AgentEvent`]s into it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    /// How long a finished turn took and what it used.
    Stats(TurnStats),
}

/// Tokens used, counting the whole prompt (cached or not) as input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount {
    pub input: u64,
    pub output: u64,
}

impl std::ops::AddAssign for TokenCount {
    fn add_assign(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnStats {
    pub duration_ms: u64,
    pub tokens: TokenCount,
    /// Known for turns run here; Claude's history doesn't record it.
    pub cost_usd: Option<f64>,
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
    #[serde(skip)]
    turn: TurnProgress,
}

/// The turn in progress. It lasts only while the turn runs, so it is
/// neither saved nor part of a thread's equality.
#[derive(Debug, Clone, Default)]
struct TurnProgress {
    started: Option<Instant>,
    /// Tokens of the turn's finished model calls.
    counted: TokenCount,
    /// The model call still running, with its id.
    call: Option<(Option<String>, TokenCount)>,
}

impl PartialEq for TurnProgress {
    fn eq(&self, _: &Self) -> bool {
        true
    }
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
            turn: TurnProgress::default(),
        }
    }

    /// How long the turn in progress has run; `None` between turns.
    pub fn turn_elapsed(&self) -> Option<Duration> {
        self.turn.started.map(|started| started.elapsed())
    }

    fn end_turn(&mut self) {
        self.turn = TurnProgress::default();
    }

    /// Tokens the turn in progress has used so far.
    pub fn turn_tokens(&self) -> TokenCount {
        let mut tokens = self.turn.counted;
        if let Some((_, call)) = self.turn.call {
            tokens += call;
        }
        tokens
    }

    /// A model call reports running totals several times; a report with a
    /// new id starts the next call.
    fn count_usage(&mut self, message_id: &Option<String>, tokens: TokenCount) {
        match &mut self.turn.call {
            Some((id, call)) if message_id.is_none() || id == message_id => {
                call.input = call.input.max(tokens.input);
                call.output = call.output.max(tokens.output);
            }
            _ => {
                if let Some((_, finished)) = self.turn.call.take() {
                    self.turn.counted += finished;
                }
                self.turn.call = Some((message_id.clone(), tokens));
            }
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
        self.turn = TurnProgress {
            started: Some(Instant::now()),
            ..TurnProgress::default()
        };
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
            AgentEvent::Usage {
                message_id,
                input_tokens,
                output_tokens,
            } => self.count_usage(
                message_id,
                TokenCount {
                    input: *input_tokens,
                    output: *output_tokens,
                },
            ),
            AgentEvent::TurnDone {
                is_error,
                result,
                cost_usd,
                duration_ms,
            } => {
                let tokens = self.turn_tokens();
                self.end_turn();
                self.push_finished(Item::Stats(TurnStats {
                    duration_ms: *duration_ms,
                    tokens,
                    cost_usd: *cost_usd,
                }));
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
        self.end_turn();
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
