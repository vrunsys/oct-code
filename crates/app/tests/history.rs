//! Existing Claude Code sessions of a project appear as threads.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_desk::app_state::AppState;
use agent_desk_core::claude::history::encode_project_dir;
use agent_desk_core::thread::{Item, ThreadStatus};
use agent_desk_test_support::fake_claude_path;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use serde_json::json;

const SESSION: &str = "10d97801-6bbf-442b-a03d-a252027fea22";

struct Fixture {
    root: tempfile::TempDir,
    claude_home: tempfile::TempDir,
    _work: tempfile::TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("repo");
        fs::create_dir(&project).unwrap();
        // Projects are stored canonical; history records the real path too.
        let project = dunce_canonical(&project);
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
            claude_home: tempfile::tempdir().unwrap(),
            _work: work,
            project,
        };
        fixture.write_session();
        fixture
    }

    fn history_dir(&self) -> PathBuf {
        let dir = self
            .claude_home
            .path()
            .join("projects")
            .join(encode_project_dir(&self.project));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What Claude Code appends when the session is continued elsewhere,
    /// such as in the VS Code extension.
    fn append_turn(&self, session: &str, question: &str, answer: &str) {
        let lines = [
            json!({"type": "user", "cwd": self.project, "message": {"role": "user",
                   "content": [{"type": "text", "text": question}]}}),
            json!({"type": "assistant", "cwd": self.project, "message": {"role": "assistant",
                   "content": [{"type": "text", "text": answer}]}}),
        ];
        let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_dir().join(format!("{session}.jsonl")))
            .unwrap();
        file.write_all(body.as_bytes()).unwrap();
    }

    fn write_session(&self) {
        let dir = self.history_dir();
        let lines = [
            json!({"type": "user", "cwd": self.project, "message": {"role": "user",
                   "content": [{"type": "text", "text": "what files are in this folder?"}]}}),
            json!({"type": "assistant", "cwd": self.project, "message": {"role": "assistant",
                   "content": [{"type": "text", "text": "Two files."}]}}),
            json!({"type": "ai-title", "aiTitle": "Folder file listing", "sessionId": SESSION}),
        ];
        let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
        fs::write(dir.join(format!("{SESSION}.jsonl")), body).unwrap();
    }

    fn load(&self, cx: &mut TestAppContext, claude: Option<PathBuf>) -> Entity<AppState> {
        let root = self.root.path().to_path_buf();
        let home = self.claude_home.path().to_path_buf();
        cx.new(|_| AppState::load_with_history(root, claude, Some(home)))
    }
}

fn dunce_canonical(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap();
    PathBuf::from(canonical.to_string_lossy().trim_start_matches(r"\\?\"))
}

#[gpui_kit::test]
fn adding_a_project_shows_its_existing_sessions(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    let project = fixture.project.clone();
    app.update(cx, |app, cx| app.add_project(&project, cx));
    cx.run_until_parked();

    app.read_with(cx, |app, _| {
        let threads = app.threads_for(&fixture.project);
        assert_eq!(threads.len(), 1);
        let thread = threads[0];
        assert_eq!(thread.title, "Folder file listing");
        assert_eq!(thread.session_id.as_deref(), Some(SESSION));
        assert_eq!(thread.status, ThreadStatus::Idle);
        assert_eq!(
            thread.items,
            [
                Item::User("what files are in this folder?".into()),
                Item::Assistant("Two files.".into())
            ]
        );
    });
}

#[gpui_kit::test]
fn existing_sessions_are_imported_once(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    let project = fixture.project.clone();
    app.update(cx, |app, cx| app.add_project(&project, cx));
    cx.run_until_parked();

    let reloaded = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    reloaded.update(cx, |app, cx| app.watch_history(cx));
    cx.run_until_parked();
    assert_eq!(reloaded.read_with(cx, |app, _| app.threads().len()), 1);
}

#[gpui_kit::test]
fn an_imported_thread_resumes_its_session(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fake_claude_path()));
    let project = fixture.project.clone();
    app.update(cx, |app, cx| app.add_project(&project, cx));
    cx.run_until_parked();

    let id = app.read_with(cx, |app, _| app.threads()[0].id.clone());
    app.update(cx, |app, cx| app.send_message(&id, "args".into(), cx));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let reply = app.read_with(cx, |app, _| {
            app.threads()[0]
                .items
                .iter()
                .rev()
                .find_map(|item| match item {
                    Item::Assistant(text) if text.contains("--resume") => Some(text.clone()),
                    _ => None,
                })
        });
        if let Some(reply) = reply {
            assert!(reply.contains(&format!("--resume {SESSION}")), "{reply}");
            break;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Long enough for the history watcher to look again.
fn after_next_poll(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
}

fn items_of(cx: &mut TestAppContext, app: &Entity<AppState>, session: &str) -> Vec<Item> {
    app.read_with(cx, |app, _| {
        app.threads()
            .iter()
            .find(|thread| thread.session_id.as_deref() == Some(session))
            .map(|thread| thread.items.clone())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn turns_added_outside_the_app_appear_while_it_runs(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    let project = fixture.project.clone();
    app.update(cx, |app, cx| {
        app.add_project(&project, cx);
        app.watch_history(cx);
    });
    cx.run_until_parked();
    assert_eq!(items_of(cx, &app, SESSION).len(), 2);

    fixture.append_turn(SESSION, "and now?", "Three files.");
    after_next_poll(cx);
    assert_eq!(
        items_of(cx, &app, SESSION).last(),
        Some(&Item::Assistant("Three files.".into()))
    );
    assert_eq!(app.read_with(cx, |app, _| app.threads().len()), 1);
}

#[gpui_kit::test]
fn sessions_started_outside_the_app_appear_while_it_runs(cx: &mut TestAppContext) {
    const OTHER: &str = "20d97801-6bbf-442b-a03d-a252027fea22";
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    let project = fixture.project.clone();
    app.update(cx, |app, cx| {
        app.add_project(&project, cx);
        app.watch_history(cx);
    });
    cx.run_until_parked();

    fixture.append_turn(OTHER, "hello", "Hi.");
    after_next_poll(cx);
    assert_eq!(items_of(cx, &app, OTHER).len(), 2);
}

#[gpui_kit::test]
fn a_restart_shows_turns_added_since_the_last_run(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    let project = fixture.project.clone();
    app.update(cx, |app, cx| app.add_project(&project, cx));
    cx.run_until_parked();
    drop(app);

    fixture.append_turn(SESSION, "and now?", "Three files.");
    let reloaded = fixture.load(cx, Some(fixture.root.path().join("no-claude")));
    reloaded.update(cx, |app, cx| app.watch_history(cx));
    cx.run_until_parked();
    assert_eq!(items_of(cx, &reloaded, SESSION).len(), 4);
}

#[gpui_kit::test]
fn a_thread_running_here_is_not_overwritten_from_history(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let fixture = Fixture::new();
    let app = fixture.load(cx, Some(fake_claude_path()));
    let project = fixture.project.clone();
    let id = app.update(cx, |app, cx| {
        app.add_project(&project, cx);
        app.watch_history(cx);
        app.new_thread(&project, cx)
    });
    app.update(cx, |app, cx| app.send_message(&id, "hi".into(), cx));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.read_with(cx, |app, _| {
        app.threads()
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .items
            .len()
    }) < 2
    {
        assert!(Instant::now() < deadline, "timed out");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }

    // fake-claude names its session `fake-session-1`; its process lives on.
    fixture.append_turn("fake-session-1", "written by Claude", "meanwhile");
    after_next_poll(cx);
    assert_eq!(
        items_of(cx, &app, "fake-session-1"),
        [Item::User("hi".into()), Item::Assistant("echo: hi".into())]
    );
}
