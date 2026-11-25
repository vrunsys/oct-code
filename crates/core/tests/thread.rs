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

#[test]
fn new_thread_records_creation_time() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(thread().created_at >= before);
}

#[test]
fn thread_saved_before_created_at_still_loads() {
    let mut value = serde_json::to_value(thread()).unwrap();
    value.as_object_mut().unwrap().remove("created_at");
    let loaded: Thread = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.created_at, 0);
}

fn delta(text: &str) -> AgentEvent {
    AgentEvent::AssistantDelta { text: text.into() }
}

fn full_text(text: &str) -> AgentEvent {
    AgentEvent::AssistantText { text: text.into() }
}

#[test]
fn streamed_text_grows_one_message_until_the_full_text_replaces_it() {
    let mut t = thread();
    t.push_user("hi");
    t.apply(&delta("Hel"));
    t.apply(&delta("lo"));
    assert_eq!(t.items.last(), Some(&Item::Assistant("Hello".into())));
    t.apply(&full_text("Hello!"));
    assert_eq!(
        t.items,
        [Item::User("hi".into()), Item::Assistant("Hello!".into())]
    );
}

#[test]
fn text_after_a_finished_block_or_a_tool_call_is_a_new_message() {
    let mut t = thread();
    t.push_user("hi");
    t.apply(&delta("one"));
    t.apply(&full_text("one"));
    t.apply(&delta("two"));
    t.apply(&full_text("two"));
    t.apply(&AgentEvent::ToolUse {
        id: "t1".into(),
        name: "Bash".into(),
        input: json!({}),
    });
    t.apply(&delta("three"));
    let texts: Vec<_> = t
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Assistant(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["one", "two", "three"]);
}

#[test]
fn unstreamed_text_is_appended_as_before() {
    let mut t = thread();
    t.push_user("hi");
    t.apply(&full_text("a"));
    t.apply(&full_text("b"));
    assert_eq!(t.items.len(), 3);
}

#[test]
fn thinking_streams_into_its_own_item_before_the_reply() {
    let mut t = thread();
    t.push_user("hi");
    t.apply(&AgentEvent::ThinkingDelta {
        text: "Check".into(),
    });
    t.apply(&AgentEvent::ThinkingDelta { text: "ing".into() });
    assert_eq!(t.items.last(), Some(&Item::Thinking("Checking".into())));
    t.apply(&AgentEvent::Thinking {
        text: "Checking.".into(),
    });
    t.apply(&delta("Yes"));
    t.apply(&full_text("Yes."));
    assert_eq!(
        t.items,
        [
            Item::User("hi".into()),
            Item::Thinking("Checking.".into()),
            Item::Assistant("Yes.".into())
        ]
    );
}

#[test]
fn reply_text_never_lands_in_a_thinking_item() {
    let mut t = thread();
    t.push_user("hi");
    t.apply(&AgentEvent::ThinkingDelta { text: "hmm".into() });
    // The whole thinking block was empty, so no Thinking event came.
    t.apply(&delta("Yes"));
    assert_eq!(
        t.items,
        [
            Item::User("hi".into()),
            Item::Thinking("hmm".into()),
            Item::Assistant("Yes".into())
        ]
    );
}

fn usage(id: Option<&str>, input: u64, output: u64) -> AgentEvent {
    AgentEvent::Usage {
        message_id: id.map(str::to_owned),
        input_tokens: input,
        output_tokens: output,
    }
}

#[test]
fn a_finished_turn_records_its_time_tokens_and_cost() {
    use agent_desk_core::thread::{TokenCount, TurnStats};

    let mut t = thread();
    t.push_user("hi");
    // One model call, reported at its start, on each block, and at its end.
    t.apply(&usage(Some("m1"), 100, 1));
    t.apply(&usage(Some("m1"), 100, 1));
    t.apply(&usage(None, 100, 40));
    assert_eq!(
        t.turn_tokens(),
        TokenCount {
            input: 100,
            output: 40
        }
    );
    // A second call in the same turn adds to the first.
    t.apply(&usage(Some("m2"), 150, 10));
    t.apply(&turn_done(false, "ok", Some(0.25)));
    assert_eq!(
        t.items.last(),
        Some(&Item::Stats(TurnStats {
            duration_ms: 1,
            tokens: TokenCount {
                input: 250,
                output: 50
            },
            cost_usd: Some(0.25),
        }))
    );
    assert_eq!(
        t.turn_tokens(),
        TokenCount::default(),
        "the next turn starts at zero"
    );
}

#[test]
fn a_turn_is_timed_from_the_message_until_it_ends() {
    let mut t = thread();
    assert_eq!(t.turn_elapsed(), None);
    t.push_user("hi");
    assert!(t.turn_elapsed().is_some());
    t.apply(&turn_done(false, "ok", None));
    assert_eq!(t.turn_elapsed(), None);

    t.push_user("again");
    t.on_exit(Some(1), "");
    assert_eq!(t.turn_elapsed(), None, "an ended process ends the turn");
}

#[test]
fn a_new_turn_counts_tokens_from_zero() {
    use agent_desk_core::thread::TokenCount;

    let mut t = thread();
    t.push_user("hi");
    t.apply(&usage(Some("m1"), 100, 5));
    t.on_exit(None, "");
    t.push_user("again");
    assert_eq!(t.turn_tokens(), TokenCount::default());
}

fn compacted_event(trigger: &str) -> AgentEvent {
    AgentEvent::Compacted {
        trigger: trigger.into(),
        pre_tokens: Some(100),
        post_tokens: Some(10),
    }
}

#[test]
fn a_compaction_becomes_a_marker_that_takes_its_summary() {
    use agent_desk_core::thread::Compaction;

    let mut t = thread();
    t.push_user("hi");
    t.apply(&compacted_event("auto"));
    t.apply(&AgentEvent::CompactSummary {
        text: "The session so far…".into(),
    });
    assert_eq!(
        t.items.last(),
        Some(&Item::Compacted(Compaction {
            trigger: "auto".into(),
            pre_tokens: Some(100),
            post_tokens: Some(10),
            summary: Some("The session so far…".into()),
        }))
    );
    // The summary is not a message from the user.
    assert_eq!(t.items.len(), 2);
}

#[test]
fn a_summary_with_no_compaction_before_it_is_dropped() {
    let mut t = thread();
    t.apply(&AgentEvent::CompactSummary { text: "x".into() });
    assert!(t.items.is_empty());
}

#[test]
fn each_compaction_keeps_its_own_summary() {
    use agent_desk_core::thread::Compaction;

    let mut t = thread();
    for (trigger, summary) in [("auto", "first"), ("manual", "second")] {
        t.apply(&compacted_event(trigger));
        t.apply(&AgentEvent::CompactSummary {
            text: summary.into(),
        });
    }
    let summaries: Vec<_> = t
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Compacted(Compaction { summary, .. }) => summary.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(summaries, ["first", "second"]);
}
