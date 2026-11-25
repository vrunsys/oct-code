use std::path::PathBuf;

use agent_desk_core::claude::{
    AgentEvent, ClaudeConfig, ClaudeSession, PermissionSetup, SessionError, SessionEvent,
};
use agent_desk_test_support::{block_on_timeout, fake_claude_path};
use async_channel::Receiver;

fn config(cwd: PathBuf, resume: Option<&str>) -> ClaudeConfig {
    ClaudeConfig {
        binary: fake_claude_path(),
        cwd,
        resume: resume.map(str::to_owned),
        model: None,
        effort: None,
        permission: PermissionSetup::FullAccess,
    }
}

/// Receives events up to and including the first one matching `stop`.
async fn events_until(
    events: &Receiver<SessionEvent>,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut seen = Vec::new();
    loop {
        let event = events
            .recv()
            .await
            .expect("session event stream ended early");
        let done = stop(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn is_turn_done(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::Agent(AgentEvent::TurnDone { .. }))
}

fn assistant_texts(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Agent(AgentEvent::AssistantText { text }) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn echo_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (_session, events) = ClaudeSession::start(&config(dir.path().into(), None)).unwrap();
    block_on_timeout(async {
        _session.send("hi").await.unwrap();
        let seen = events_until(&events, is_turn_done).await;
        assert!(
            matches!(
                seen.as_slice(),
                [
                    SessionEvent::Agent(AgentEvent::Init { session_id, .. }),
                    SessionEvent::Agent(AgentEvent::AssistantText { text }),
                    SessionEvent::Agent(AgentEvent::TurnDone { .. }),
                ] if session_id == "fake-session-1" && text == "echo: hi"
            ),
            "unexpected events: {seen:?}"
        );
    });
}

#[test]
fn two_messages_one_process() {
    let dir = tempfile::tempdir().unwrap();
    let (session, events) = ClaudeSession::start(&config(dir.path().into(), None)).unwrap();
    block_on_timeout(async {
        session.send("a").await.unwrap();
        let mut seen = events_until(&events, is_turn_done).await;
        session.send("b").await.unwrap();
        seen.extend(events_until(&events, is_turn_done).await);

        assert_eq!(assistant_texts(&seen), ["echo: a", "echo: b"]);
        let inits = seen
            .iter()
            .filter(|e| matches!(e, SessionEvent::Agent(AgentEvent::Init { .. })))
            .count();
        assert_eq!(inits, 1);
    });
}

#[test]
fn garbage_line_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (session, events) = ClaudeSession::start(&config(dir.path().into(), None)).unwrap();
    block_on_timeout(async {
        session.send("garbage").await.unwrap();
        let seen = events_until(&events, is_turn_done).await;
        assert_eq!(assistant_texts(&seen), ["echo: garbage"]);
    });
}

#[test]
fn crash_reports_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (session, events) = ClaudeSession::start(&config(dir.path().into(), None)).unwrap();
    block_on_timeout(async {
        session.send("crash").await.unwrap();
        let seen = events_until(&events, |e| matches!(e, SessionEvent::Exited { .. })).await;
        match seen.last() {
            Some(SessionEvent::Exited { code, stderr_tail }) => {
                assert_eq!(*code, Some(1));
                assert!(stderr_tail.contains("boom"), "stderr_tail: {stderr_tail:?}");
            }
            other => panic!("expected Exited, got {other:?}"),
        }
    });
}

#[test]
fn missing_binary_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path().into(), None);
    cfg.binary = PathBuf::from("/definitely/not/here");
    assert!(matches!(
        ClaudeSession::start(&cfg),
        Err(SessionError::NotFound(_))
    ));
}

#[test]
fn missing_cwd_is_cwd_missing() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().to_path_buf();
    drop(dir);
    assert!(matches!(
        ClaudeSession::start(&config(gone.clone(), None)),
        Err(SessionError::CwdMissing(path)) if path == gone
    ));
}

#[test]
fn resume_passes_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (session, events) = ClaudeSession::start(&config(dir.path().into(), Some("abc"))).unwrap();
    block_on_timeout(async {
        session.send("args").await.unwrap();
        let seen = events_until(&events, is_turn_done).await;
        assert!(matches!(
            seen.first(),
            Some(SessionEvent::Agent(AgentEvent::Init { session_id, .. })) if session_id == "abc"
        ));
        let texts = assistant_texts(&seen);
        assert!(
            texts.iter().any(|t| t.contains("--resume abc")),
            "texts: {texts:?}"
        );
    });
}
