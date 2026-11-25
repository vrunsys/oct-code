use std::path::PathBuf;

use agent_desk_core::claude::AgentEvent;
use agent_desk_core::thread::{Item, Thread, ThreadStatus, ToolOutcome};
use serde_json::json;

fn thread() -> Thread {
    Thread::new(PathBuf::from("/tmp/project"))
}

fn turn_done(is_error: bool, result: &str, cost_usd: Option<f64>) -> AgentEvent {
    AgentEvent::TurnDone {
        is_error,
        result: result.into(),
        cost_usd,
        duration_ms: 1,
    }
}

#[test]
fn new_thread_is_idle_and_untitled() {
    let t = thread();
    assert_eq!(t.status, ThreadStatus::Idle);
    assert_eq!(t.title, "New thread");
    assert!(t.items.is_empty());
}

#[test]
fn first_user_message_sets_title_truncated() {
    let mut t = thread();
    t.push_user(&"é".repeat(100));
    assert_eq!(t.title.chars().count(), 60);
    assert_eq!(t.status, ThreadStatus::Running);
}

#[test]
fn later_user_messages_keep_title() {
    let mut t = thread();
    t.push_user("first");
    t.push_user("second");
    assert_eq!(t.title, "first");
    assert_eq!(
        t.items,
        vec![Item::User("first".into()), Item::User("second".into())]
    );
}

#[test]
fn init_sets_session_id() {
    let mut t = thread();
    t.apply(&AgentEvent::Init {
        session_id: "s1".into(),
        model: "m".into(),
    });
    assert_eq!(t.session_id.as_deref(), Some("s1"));
}

#[test]
fn assistant_text_appends_item() {
    let mut t = thread();
    t.apply(&AgentEvent::AssistantText {
        text: "hello".into(),
    });
    assert_eq!(t.items, vec![Item::Assistant("hello".into())]);
}

#[test]
fn tool_result_attaches_to_matching_tool() {
    let mut t = thread();
    t.apply(&AgentEvent::ToolUse {
        id: "t1".into(),
        name: "Bash".into(),
        input: json!({"command": "ls"}),
    });
    t.apply(&AgentEvent::ToolResult {
        tool_use_id: "t1".into(),
        content: "x".into(),
        is_error: true,
    });
    match &t.items[..] {
        [Item::Tool { id, outcome, .. }] => {
            assert_eq!(id, "t1");
            assert_eq!(outcome, &Some(ToolOutcome::Error("x".into())));
        }
        other => panic!("expected one Tool item, got {other:?}"),
    }
}

#[test]
fn tool_result_for_unknown_id_is_ignored() {
    let mut t = thread();
    t.apply(&AgentEvent::AssistantText { text: "a".into() });
    let before = t.items.clone();
    t.apply(&AgentEvent::ToolResult {
        tool_use_id: "nope".into(),
        content: "x".into(),
        is_error: false,
    });
    assert_eq!(t.items, before);
}

#[test]
fn turn_done_sets_idle_and_adds_cost() {
    let mut t = thread();
    t.push_user("go");
    t.apply(&turn_done(false, "ok", Some(0.01)));
    t.push_user("again");
    t.apply(&turn_done(false, "ok", Some(0.01)));
    assert!((t.cost_usd - 0.02).abs() < 1e-9);
    assert_eq!(t.status, ThreadStatus::Idle);
}

#[test]
fn turn_done_error_sets_failed() {
    let mut t = thread();
    t.push_user("go");
    t.apply(&turn_done(true, "rate limited", None));
    assert_eq!(t.status, ThreadStatus::Failed("rate limited".into()));
}

#[test]
fn exit_while_running_sets_failed() {
    let mut t = thread();
    t.push_user("go");
    t.on_exit(Some(1), "boom");
    assert_eq!(
        t.status,
        ThreadStatus::Failed("Agent exited (code 1): boom".into())
    );
}

#[test]
fn exit_while_idle_stays_idle() {
    let mut t = thread();
    t.on_exit(Some(0), "");
    assert_eq!(t.status, ThreadStatus::Idle);
}
