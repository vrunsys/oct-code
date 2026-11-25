use agent_desk_core::claude::{AgentEvent, ParseError, parse_line};

const BASIC: &str = include_str!("fixtures/claude_basic.jsonl");
const TOOL: &str = include_str!("fixtures/claude_tool.jsonl");
/// Captured with `--include-partial-messages`.
const PARTIAL: &str = include_str!("fixtures/claude_partial.jsonl");

fn parse_all(fixture: &str) -> Vec<AgentEvent> {
    fixture
        .lines()
        .flat_map(|line| parse_line(line).expect("fixture line parses"))
        .collect()
}

#[test]
fn init_line_yields_session_and_model() {
    // Real output can start with SessionStart hook events before `init`;
    // those must parse as Unknown, and the init line must yield exactly Init.
    let mut lines = BASIC.lines();
    let init = lines
        .by_ref()
        .map(|line| parse_line(line).unwrap())
        .find(|events| !matches!(events.as_slice(), [AgentEvent::Unknown { .. }]))
        .expect("an init line");
    match init.as_slice() {
        [AgentEvent::Init { session_id, model }] => {
            assert!(!session_id.is_empty());
            assert!(!model.is_empty());
        }
        other => panic!("expected [Init], got {other:?}"),
    }
}

#[test]
fn assistant_text_block_yields_text() {
    let texts: Vec<_> = parse_all(BASIC)
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::AssistantText { text } => Some(text),
            _ => None,
        })
        .collect();
    assert!(!texts.is_empty());
    assert!(texts.iter().all(|t| !t.is_empty()));
}

#[test]
fn tool_fixture_yields_tool_use_then_result() {
    let events = parse_all(TOOL);
    let (use_index, id) = events
        .iter()
        .enumerate()
        .find_map(|(i, e)| match e {
            AgentEvent::ToolUse { id, .. } => Some((i, id.clone())),
            _ => None,
        })
        .expect("a ToolUse event");
    let result_index = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolResult { tool_use_id, .. } if *tool_use_id == id))
        .expect("a matching ToolResult");
    assert!(result_index > use_index);
}

#[test]
fn result_line_yields_turn_done() {
    let last = BASIC.lines().last().unwrap();
    match parse_line(last).unwrap().as_slice() {
        [
            AgentEvent::TurnDone {
                is_error: false,
                duration_ms,
                ..
            },
        ] => assert!(*duration_ms > 0),
        other => panic!("expected [TurnDone], got {other:?}"),
    }
}

#[test]
fn unknown_type_is_not_an_error() {
    assert_eq!(
        parse_line(r#"{"type":"future_thing"}"#),
        Ok(vec![AgentEvent::Unknown {
            raw_type: "future_thing".into()
        }])
    );
}

#[test]
fn blank_line_yields_nothing() {
    assert_eq!(parse_line("  "), Ok(vec![]));
}

#[test]
fn malformed_json_is_err() {
    assert!(matches!(parse_line("{nope"), Err(ParseError::Json(_))));
}

#[test]
fn partial_text_arrives_as_deltas_before_the_full_text() {
    let events = parse_all(PARTIAL);
    let streamed: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::AssistantDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let first_delta = events
        .iter()
        .position(|e| matches!(e, AgentEvent::AssistantDelta { .. }))
        .expect("a delta");
    let (full_at, full) = events
        .iter()
        .enumerate()
        .find_map(|(ix, e)| match e {
            AgentEvent::AssistantText { text } => Some((ix, text.clone())),
            _ => None,
        })
        .expect("the full text");
    assert!(first_delta < full_at);
    assert_eq!(streamed, full);
}

#[test]
fn stream_events_other_than_text_yield_nothing() {
    for line in [
        r#"{"type":"stream_event","event":{"type":"message_stop"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
    ] {
        assert_eq!(parse_line(line), Ok(Vec::new()), "{line}");
    }
}
