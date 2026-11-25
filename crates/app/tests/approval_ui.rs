//! Approval requests reaching a thread, the person's decision going back, and
//! the per-thread permission mode.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_desk::app_state::AppState;
use agent_desk_core::approval::{
    ApprovalDecision, ApprovalReply, ApprovalRequest, ApproverEndpoint,
};
use agent_desk_core::thread::{
    ApprovalState, Item, PermissionMode, Thread, ThreadId, ThreadStatus,
};
use agent_desk_test_support::fake_claude_path;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use serde_json::json;

struct Fixture {
    root: tempfile::TempDir,
    _work: tempfile::TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("repo");
        fs::create_dir(&project).unwrap();
        Self {
            root: tempfile::tempdir().unwrap(),
            _work: work,
            project,
        }
    }

    fn endpoint() -> ApproverEndpoint {
        ApproverEndpoint {
            exe: PathBuf::from("/usr/bin/agent-desk"),
            socket: "agent-desk-test".into(),
            token: "secret".into(),
        }
    }

    /// A loaded app with fake-claude and one selected thread in the project.
    fn start(
        &self,
        cx: &mut TestAppContext,
        approver: Option<ApproverEndpoint>,
    ) -> (Entity<AppState>, ThreadId) {
        cx.executor().allow_parking();
        let root = self.root.path().to_path_buf();
        let app = cx.new(|_| {
            let app = AppState::load_with_history(root, Some(fake_claude_path()), None);
            match approver {
                Some(endpoint) => app.with_approver(endpoint),
                None => app,
            }
        });
        let project = self.project.clone();
        let id = app.update(cx, |app, cx| {
            app.add_project(&project, cx);
            app.new_thread(&project, cx)
        });
        (app, id)
    }
}

fn request(thread_id: &ThreadId, request_id: &str) -> ApprovalRequest {
    ApprovalRequest {
        request_id: request_id.into(),
        thread_id: thread_id.clone(),
        tool_name: "Bash".into(),
        input: json!({"command": "rm hello.txt"}),
    }
}

fn thread(cx: &mut TestAppContext, app: &Entity<AppState>, id: &ThreadId) -> Thread {
    app.read_with(cx, |app, _| {
        app.threads().iter().find(|t| &t.id == id).unwrap().clone()
    })
}

fn approval_state(thread: &Thread, request_id: &str) -> Option<ApprovalState> {
    thread.items.iter().find_map(|item| match item {
        Item::Approval {
            request_id: id,
            state,
            ..
        } if id == request_id => Some(*state),
        _ => None,
    })
}

/// Sends `text` and waits until its reply has arrived.
fn send_and_wait(
    cx: &mut TestAppContext,
    app: &Entity<AppState>,
    id: &ThreadId,
    text: &str,
) -> Thread {
    app.update(cx, |app, cx| app.send_message(id, text.into(), cx));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let current = thread(cx, app, id);
        if current.status != ThreadStatus::Running {
            return current;
        }
        assert!(Instant::now() < deadline, "timed out: {current:#?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn last_assistant(thread: &Thread) -> String {
    thread
        .items
        .iter()
        .rev()
        .find_map(|item| match item {
            Item::Assistant(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[gpui_kit::test]
fn request_shows_pending_then_allow(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    let (reply, decided) = ApprovalReply::channel();
    app.update(cx, |app, cx| {
        app.on_approval_request(request(&id, "r1"), reply, cx)
    });
    assert_eq!(
        approval_state(&thread(cx, &app, &id), "r1"),
        Some(ApprovalState::Pending)
    );
    assert!(
        decided.is_empty(),
        "nothing is decided until the person clicks"
    );

    app.update(cx, |app, cx| app.approve("r1", true, cx));
    assert_eq!(
        approval_state(&thread(cx, &app, &id), "r1"),
        Some(ApprovalState::Allowed)
    );
    assert_eq!(decided.try_recv(), Ok(ApprovalDecision::Allow));
}

#[gpui_kit::test]
fn deny_sends_denied_by_user(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    let (reply, decided) = ApprovalReply::channel();
    app.update(cx, |app, cx| {
        app.on_approval_request(request(&id, "r1"), reply, cx);
        app.approve("r1", false, cx);
    });
    assert_eq!(
        approval_state(&thread(cx, &app, &id), "r1"),
        Some(ApprovalState::Denied)
    );
    assert_eq!(
        decided.try_recv(),
        Ok(ApprovalDecision::deny("Denied by user"))
    );
}

#[gpui_kit::test]
fn unknown_thread_is_denied(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, _) = fixture.start(cx, Some(Fixture::endpoint()));
    let (reply, decided) = ApprovalReply::channel();
    app.update(cx, |app, cx| {
        app.on_approval_request(request(&ThreadId::new(), "r1"), reply, cx)
    });
    assert_eq!(
        decided.try_recv(),
        Ok(ApprovalDecision::deny("Unknown thread"))
    );
}

#[gpui_kit::test]
fn a_repeated_request_id_is_denied_and_the_first_stays_pending(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    let (first, first_decided) = ApprovalReply::channel();
    let (second, second_decided) = ApprovalReply::channel();
    app.update(cx, |app, cx| {
        app.on_approval_request(request(&id, "r1"), first, cx);
        app.on_approval_request(request(&id, "r1"), second, cx);
    });
    assert!(matches!(
        second_decided.try_recv(),
        Ok(ApprovalDecision::Deny { .. })
    ));
    assert!(first_decided.is_empty());
    let approvals = thread(cx, &app, &id)
        .items
        .iter()
        .filter(|item| matches!(item, Item::Approval { .. }))
        .count();
    assert_eq!(approvals, 1);
}

#[gpui_kit::test]
fn stop_denies_pending(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    send_and_wait(cx, &app, &id, "hi");
    let (reply, decided) = ApprovalReply::channel();
    app.update(cx, |app, cx| {
        app.on_approval_request(request(&id, "r1"), reply, cx);
        app.stop(&id, cx);
    });
    assert_eq!(
        decided.try_recv(),
        Ok(ApprovalDecision::deny("Session stopped"))
    );
    assert_eq!(
        approval_state(&thread(cx, &app, &id), "r1"),
        Some(ApprovalState::Denied)
    );
}

#[gpui_kit::test]
fn supervised_writes_mcp_config(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    let reply = last_assistant(&send_and_wait(cx, &app, &id, "args"));
    let config = fixture
        .root
        .path()
        .join("mcp")
        .join(format!("{}.json", id.0));
    assert!(
        reply.contains("--permission-prompt-tool mcp__desk__approve"),
        "{reply}"
    );
    assert!(
        reply.contains(&format!("--mcp-config {}", config.display())),
        "{reply}"
    );
    assert!(!reply.contains("bypassPermissions"), "{reply}");
    assert!(config.is_file());
}

#[gpui_kit::test]
fn a_supervised_thread_without_approvals_does_not_start(cx: &mut TestAppContext) {
    // Failing closed: no approval server must never mean "run unsupervised".
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, None);
    let thread = send_and_wait(cx, &app, &id, "args");
    assert!(
        matches!(&thread.status, ThreadStatus::Failed(message) if message.contains("pprovals")),
        "{:?}",
        thread.status
    );
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));
}

#[gpui_kit::test]
fn full_access_skips_approvals(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, None);
    app.update(cx, |app, cx| {
        app.set_mode(&id, PermissionMode::FullAccess, cx)
    });
    let reply = last_assistant(&send_and_wait(cx, &app, &id, "args"));
    assert!(
        reply.contains("--permission-mode bypassPermissions"),
        "{reply}"
    );
    assert!(!reply.contains("--permission-prompt-tool"), "{reply}");
    assert_eq!(
        thread(cx, &app, &id).permission_mode,
        PermissionMode::FullAccess
    );
}

#[gpui_kit::test]
fn changing_mode_ends_the_live_session(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    send_and_wait(cx, &app, &id, "hi");
    assert!(app.read_with(cx, |app, _| app.is_live(&id)));
    app.update(cx, |app, cx| {
        app.set_mode(&id, PermissionMode::FullAccess, cx)
    });
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));
    let reply = last_assistant(&send_and_wait(cx, &app, &id, "args"));
    assert!(reply.contains("--resume fake-session-1"), "{reply}");
    assert!(reply.contains("bypassPermissions"), "{reply}");
}

#[gpui_kit::test]
fn a_pending_approval_does_not_survive_a_restart(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx, Some(Fixture::endpoint()));
    let (reply, _decided) = ApprovalReply::channel();
    app.update(cx, |app, cx| {
        app.on_approval_request(request(&id, "r1"), reply, cx)
    });
    drop(app);

    let (reloaded, _) = fixture.start(cx, Some(Fixture::endpoint()));
    let thread = thread(cx, &reloaded, &id);
    assert_eq!(approval_state(&thread, "r1"), Some(ApprovalState::Denied));
}
