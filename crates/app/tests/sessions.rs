//! AppState driving fake-claude as a real child process.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_desk::app_state::AppState;
use agent_desk_core::store::ThreadStore;
use agent_desk_core::thread::{Item, Thread, ThreadId, ThreadStatus};
use agent_desk_test_support::fake_claude_path;
use gpui_kit::{AppContext as _, Entity, TestAppContext};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Fixture {
    root: tempfile::TempDir,
    _work: tempfile::TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("repo");
        fs::create_dir(&project).unwrap();
        Self {
            root,
            _work: work,
            project,
        }
    }

    fn load(&self, cx: &mut TestAppContext, claude: Option<PathBuf>) -> Entity<AppState> {
        cx.executor().allow_parking();
        let root = self.root.path().to_path_buf();
        cx.new(|_| AppState::load(root, claude))
    }

    /// A loaded app with fake-claude, this project and one selected thread.
    fn start(&self, cx: &mut TestAppContext) -> (Entity<AppState>, ThreadId) {
        let app = self.load(cx, Some(fake_claude_path()));
        let project = self.project.clone();
        let id = app.update(cx, |app, cx| {
            app.add_project(&project, cx);
            app.new_thread(&project, cx)
        });
        (app, id)
    }
}

fn send(cx: &mut TestAppContext, app: &Entity<AppState>, id: &ThreadId, text: &str) {
    app.update(cx, |app, cx| app.send_message(id, text.into(), cx));
}

fn thread(cx: &mut TestAppContext, app: &Entity<AppState>, id: &ThreadId) -> Thread {
    app.read_with(cx, |app, _| {
        app.threads().iter().find(|t| &t.id == id).unwrap().clone()
    })
}

/// Pumps GPUI until `done` holds for the thread, or fails after [`TIMEOUT`].
fn wait_for(
    cx: &mut TestAppContext,
    app: &Entity<AppState>,
    id: &ThreadId,
    done: impl Fn(&Thread) -> bool,
) -> Thread {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        cx.run_until_parked();
        let current = thread(cx, app, id);
        if done(&current) {
            return current;
        }
        assert!(
            Instant::now() < deadline,
            "timed out; thread is {current:#?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn idle_with_items(len: usize) -> impl Fn(&Thread) -> bool {
    move |t| t.status == ThreadStatus::Idle && t.items.len() == len
}

fn last_assistant(thread: &Thread) -> &str {
    thread
        .items
        .iter()
        .rev()
        .find_map(|item| match item {
            Item::Assistant(text) => Some(text.as_str()),
            _ => None,
        })
        .expect("an assistant reply")
}

fn is_failed_with(needle: &'static str) -> impl Fn(&Thread) -> bool {
    move |t| matches!(&t.status, ThreadStatus::Failed(message) if message.contains(needle))
}

#[gpui_kit::test]
fn send_streams_reply(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    send(cx, &app, &id, "hi");
    let thread = wait_for(cx, &app, &id, idle_with_items(2));
    assert_eq!(
        thread.items,
        [Item::User("hi".into()), Item::Assistant("echo: hi".into())]
    );
}

#[gpui_kit::test]
fn reply_is_persisted(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    send(cx, &app, &id, "hi");
    wait_for(cx, &app, &id, idle_with_items(2));

    let saved = ThreadStore::new(fixture.root.path().into()).load_all();
    let saved = saved.iter().find(|t| t.id == id).unwrap();
    assert_eq!(saved.items.len(), 2);
    assert_eq!(saved.session_id.as_deref(), Some("fake-session-1"));
}

#[gpui_kit::test]
fn restart_resumes_session(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    send(cx, &app, &id, "hi");
    wait_for(cx, &app, &id, idle_with_items(2));

    let reloaded = fixture.load(cx, Some(fake_claude_path()));
    send(cx, &reloaded, &id, "args");
    let thread = wait_for(cx, &reloaded, &id, idle_with_items(4));
    assert!(
        last_assistant(&thread).contains("--resume fake-session-1"),
        "{thread:#?}"
    );
}

#[gpui_kit::test]
fn crash_fails_then_recovers(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    send(cx, &app, &id, "crash");
    wait_for(cx, &app, &id, is_failed_with("boom"));
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));

    send(cx, &app, &id, "hi");
    let thread = wait_for(cx, &app, &id, |t| t.status == ThreadStatus::Idle);
    assert_eq!(last_assistant(&thread), "echo: hi");
}

#[gpui_kit::test]
fn missing_project_folder_fails_cleanly(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    fs::remove_dir(&fixture.project).unwrap();

    send(cx, &app, &id, "hi");
    let thread = thread(cx, &app, &id);
    assert_eq!(
        thread.status,
        ThreadStatus::Failed(format!(
            "Project folder not found: {}",
            fixture.project.display()
        ))
    );
    // The message is kept, so nothing the user typed is lost.
    assert_eq!(thread.items, [Item::User("hi".into())]);
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));
}

#[gpui_kit::test]
fn stop_kills_session(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    send(cx, &app, &id, "hi");
    wait_for(cx, &app, &id, idle_with_items(2));
    assert!(app.read_with(cx, |app, _| app.is_live(&id)));

    app.update(cx, |app, cx| app.stop(&id, cx));
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));
    assert_eq!(thread(cx, &app, &id).status, ThreadStatus::Idle);
}

#[gpui_kit::test]
fn missing_claude_fails_with_hint(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fixture.root.path().join("no-claude-here")));
    let project = fixture.project.clone();
    let id = app.update(cx, |app, cx| {
        app.add_project(&project, cx);
        app.new_thread(&project, cx)
    });
    send(cx, &app, &id, "hi");
    let thread = thread(cx, &app, &id);
    assert!(
        is_failed_with("Claude Code not found")(&thread),
        "{thread:#?}"
    );
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));
}

#[gpui_kit::test]
fn stop_then_send_ignores_the_old_sessions_exit(cx: &mut TestAppContext) {
    // The killed process reports Exited after the new one has started; that
    // stale event must not fail the new turn.
    let fixture = Fixture::new();
    let (app, id) = fixture.start(cx);
    send(cx, &app, &id, "hi");
    wait_for(cx, &app, &id, idle_with_items(2));

    app.update(cx, |app, cx| {
        app.stop(&id, cx);
        app.send_message(&id, "again".into(), cx);
    });
    let thread = wait_for(cx, &app, &id, idle_with_items(4));
    assert_eq!(last_assistant(&thread), "echo: again");
    // Let any late event from the killed process arrive, then check again.
    std::thread::sleep(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(self::thread(cx, &app, &id).status, ThreadStatus::Idle);
}

#[gpui_kit::test]
fn unsafe_saved_session_id_is_not_resumed(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let mut saved = Thread::new(fixture.project.clone());
    saved.session_id = Some("--dangerously-skip-permissions".into());
    let id = saved.id.clone();
    ThreadStore::new(fixture.root.path().into())
        .save(&saved)
        .unwrap();

    let app = fixture.load(cx, Some(fake_claude_path()));
    send(cx, &app, &id, "args");
    let thread = wait_for(cx, &app, &id, idle_with_items(2));
    assert!(!last_assistant(&thread).contains("--resume"), "{thread:#?}");
    assert!(
        !last_assistant(&thread).contains("dangerously"),
        "{thread:#?}"
    );
}
