//! One conversation with an agent, and the reducer that folds
//! [`AgentEvent`]s into it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolOutcome {
    Ok(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Item {
    User(String),
    Assistant(String),
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
    }

    pub fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Init { session_id, .. } => self.session_id = Some(session_id.clone()),
            AgentEvent::AssistantText { text } => self.items.push(Item::Assistant(text.clone())),
            AgentEvent::ToolUse { id, name, input } => self.items.push(Item::Tool {
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

    /// The agent process ended. Only an exit in the middle of a turn is a failure.
    pub fn on_exit(&mut self, code: Option<i32>, stderr_tail: &str) {
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
