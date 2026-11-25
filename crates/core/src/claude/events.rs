//! Claude Code `--output-format stream-json` lines to [`AgentEvent`]s.
//!
//! Fixtures in `tests/fixtures/` were captured with `claude --version` 2.1.332
//! and scrubbed with `scripts/scrub-claude-fixture.py`.
//!
//! This is the only module that knows Claude's JSON shapes; the rest of the
//! app sees [`AgentEvent`] alone.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One thing the agent did, independent of the agent's wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentEvent {
    Init {
        session_id: String,
        model: String,
    },
    /// The next piece of a text block still being written. The whole block
    /// follows as [`AgentEvent::AssistantText`] once it is done.
    AssistantDelta {
        text: String,
    },
    /// A finished text block.
    AssistantText {
        text: String,
    },
    /// The next piece of Claude's thinking summary, as with
    /// [`AgentEvent::AssistantDelta`]; [`AgentEvent::Thinking`] follows.
    ThinkingDelta {
        text: String,
    },
    /// A finished thinking summary. Blocks whose thinking is omitted have no
    /// text and yield no event.
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Tokens one model call has used so far. A call reports several times
    /// as it runs, each with its running totals; `message_id` is `None` for
    /// a report on the call in progress.
    Usage {
        message_id: Option<String>,
        /// The whole prompt, including the part read from or written to
        /// the cache.
        input_tokens: u64,
        output_tokens: u64,
    },
    TurnDone {
        is_error: bool,
        result: String,
        cost_usd: Option<f64>,
        duration_ms: u64,
    },
    /// A line type this version does not understand. Not an error: newer
    /// Claude versions add types, and the session must keep running.
    Unknown {
        raw_type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Json(String),
    MissingField(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Json(error) => write!(f, "invalid JSON: {error}"),
            ParseError::MissingField(field) => write!(f, "missing field `{field}`"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses one stdout line. A blank line yields no events; one `assistant`
/// line can carry several content blocks and so several events.
pub fn parse_line(line: &str) -> Result<Vec<AgentEvent>, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(line).map_err(|e| ParseError::Json(e.to_string()))?;
    let kind = str_field(&value, "type").ok_or(ParseError::MissingField("type"))?;

    match kind {
        "system" if str_field(&value, "subtype") == Some("init") => Ok(vec![AgentEvent::Init {
            session_id: required_str(&value, "session_id")?,
            model: required_str(&value, "model")?,
        }]),
        "assistant" => content_blocks(&value)?
            .iter()
            .filter_map(|block| match str_field(block, "type") {
                Some("text") => {
                    Some(required_str(block, "text").map(|text| AgentEvent::AssistantText { text }))
                }
                Some("tool_use") => Some(parse_tool_use(block)),
                Some("thinking") => str_field(block, "thinking")
                    .filter(|text| !text.is_empty())
                    .map(|text| {
                        Ok(AgentEvent::Thinking {
                            text: text.to_owned(),
                        })
                    }),
                _ => None,
            })
            .chain(usage(value.get("message"), value.pointer("/message/usage")).map(Ok))
            .collect(),
        "user" => content_blocks(&value)?
            .iter()
            .filter(|block| str_field(block, "type") == Some("tool_result"))
            .map(parse_tool_result)
            .collect(),
        // Sent with `--include-partial-messages`. Text and thinking are shown
        // as they arrive; tool calls wait for their finished block.
        "stream_event" => Ok(stream_delta(&value)
            .or_else(|| stream_usage(&value))
            .into_iter()
            .collect()),
        "result" => Ok(vec![AgentEvent::TurnDone {
            is_error: value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            result: str_field(&value, "result").unwrap_or_default().to_owned(),
            cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
            duration_ms: value
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }]),
        other => Ok(vec![AgentEvent::Unknown {
            raw_type: other.to_owned(),
        }]),
    }
}

fn stream_delta(value: &Value) -> Option<AgentEvent> {
    let event = value.get("event")?;
    if str_field(event, "type") != Some("content_block_delta") {
        return None;
    }
    let delta = event.get("delta")?;
    let (text, event): (_, fn(String) -> AgentEvent) = match str_field(delta, "type")? {
        "text_delta" => (str_field(delta, "text")?, |text| {
            AgentEvent::AssistantDelta { text }
        }),
        "thinking_delta" => (str_field(delta, "thinking")?, |text| {
            AgentEvent::ThinkingDelta { text }
        }),
        _ => return None,
    };
    (!text.is_empty()).then(|| event(text.to_owned()))
}

/// Usage reported as a model call starts (with its id) or ends (without).
fn stream_usage(value: &Value) -> Option<AgentEvent> {
    let event = value.get("event")?;
    match str_field(event, "type")? {
        "message_start" => usage(event.get("message"), event.pointer("/message/usage")),
        "message_delta" => usage(None, event.get("usage")),
        _ => None,
    }
}

fn usage(message: Option<&Value>, usage: Option<&Value>) -> Option<AgentEvent> {
    let usage = usage?;
    let count = |key| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    Some(AgentEvent::Usage {
        message_id: message
            .and_then(|message| str_field(message, "id"))
            .map(str::to_owned),
        input_tokens: count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens"),
        output_tokens: count("output_tokens"),
    })
}

fn parse_tool_use(block: &Value) -> Result<AgentEvent, ParseError> {
    Ok(AgentEvent::ToolUse {
        id: required_str(block, "id")?,
        name: required_str(block, "name")?,
        input: block.get("input").cloned().unwrap_or(Value::Null),
    })
}

fn parse_tool_result(block: &Value) -> Result<AgentEvent, ParseError> {
    let content = match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| str_field(part, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    Ok(AgentEvent::ToolResult {
        tool_use_id: required_str(block, "tool_use_id")?,
        content,
        is_error: block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn content_blocks(value: &Value) -> Result<&[Value], ParseError> {
    match value.pointer("/message/content") {
        Some(Value::Array(blocks)) => Ok(blocks),
        // A plain-string user message carries no tool results.
        Some(Value::String(_)) => Ok(&[]),
        _ => Err(ParseError::MissingField("message.content")),
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn required_str(value: &Value, key: &'static str) -> Result<String, ParseError> {
    str_field(value, key)
        .map(str::to_owned)
        .ok_or(ParseError::MissingField(key))
}
