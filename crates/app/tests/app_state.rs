use std::fs;
use std::path::{Path, PathBuf};

use agent_desk::app_state::AppState;
use agent_desk_core::detect::AgentStatus;
use agent_desk_core::store::{ProjectList, ThreadStore};
use agent_desk_core::thread::{Item, Thread, ThreadStatus};
use gpui_kit::{AppContext as _, Entity, TestAppContext};

/// Loads app state with a Claude override that does not exist, so tests never
/// run the real `claude` binary.
fn load(root: &Path, cx: &mut TestAppContext) -> Entity<AppState> {
    let missing_claude = root.join("no-claude-here");
    let root = root.to_path_buf();
    cx.new(|_| AppState::load(root, Some(missing_claude)))
}

fn project_dir(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir(&dir).unwrap();
    dir
}

#[gpui_kit::test]
fn loads_existing_projects_and_threads(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = project_dir(work.path(), "repo");
    ProjectList::load(root.path().into()).add(&project).unwrap();
    ThreadStore::new(root.path().into())
        .save(&Thread::new(project))
        .unwrap();

    let app = load(root.path(), cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.projects().len(), 1);
        assert_eq!(app.threads().len(), 1);
    });
}

#[gpui_kit::test]
fn add_project_appears(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = project_dir(work.path(), "repo");
    let app = load(root.path(), cx);

    app.update(cx, |app, cx| app.add_project(&project, cx));
    app.read_with(cx, |app, _| {
        assert_eq!(app.projects().len(), 1);
        assert_eq!(app.last_error(), None);
    });
}

#[gpui_kit::test]
fn add_bad_project_sets_error(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.txt");
    fs::write(&file, "x").unwrap();
    let app = load(root.path(), cx);

    app.update(cx, |app, cx| app.add_project(&file, cx));
    app.read_with(cx, |app, _| {
        let error = app.last_error().expect("an error");
        assert!(error.contains("not a directory"), "{error}");
        assert!(app.projects().is_empty());
    });
}

#[gpui_kit::test]
fn new_thread_is_selected_and_saved(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = project_dir(work.path(), "repo");
    let app = load(root.path(), cx);

    let id = app.update(cx, |app, cx| {
        app.add_project(&project, cx);
        app.new_thread(&project, cx)
    });
    app.read_with(cx, |app, _| assert_eq!(app.selected(), Some(&id)));
    let saved = ThreadStore::new(root.path().into()).load_all();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, id);
}

#[gpui_kit::test]
fn threads_for_filters_by_project(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let first = project_dir(work.path(), "first");
    let second = project_dir(work.path(), "second");
    let app = load(root.path(), cx);

    let (older, newer, other) = app.update(cx, |app, cx| {
        app.add_project(&first, cx);
        app.add_project(&second, cx);
        let older = app.new_thread(&first, cx);
        let newer = app.new_thread(&first, cx);
        let other = app.new_thread(&second, cx);
        (older, newer, other)
    });
    app.read_with(cx, |app, _| {
        let ids: Vec<_> = app
            .threads_for(&first)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(ids, [newer, older], "newest first, only this project");
        assert_eq!(app.threads_for(&second)[0].id, other);
    });
}

#[gpui_kit::test]
fn claude_missing_is_reported(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let app = load(root.path(), cx);
    app.read_with(cx, |app, _| {
        assert!(!matches!(app.claude(), AgentStatus::Installed { .. }));
    });
}

#[gpui_kit::test]
fn send_message_appends_and_saves(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = project_dir(work.path(), "repo");
    let app = load(root.path(), cx);

    let id = app.update(cx, |app, cx| {
        app.add_project(&project, cx);
        let id = app.new_thread(&project, cx);
        app.send_message(&id, "fix the login bug".into(), cx);
        id
    });
    let saved = ThreadStore::new(root.path().into()).load_all();
    assert_eq!(saved[0].id, id);
    assert_eq!(saved[0].items, [Item::User("fix the login bug".into())]);
    assert_eq!(saved[0].title, "fix the login bug");
}

#[gpui_kit::test]
fn load_resets_running_threads_to_idle(cx: &mut TestAppContext) {
    // No agent process survives a restart, so a thread saved mid-turn must not
    // reopen as Running with a disabled composer.
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = project_dir(work.path(), "repo");
    let mut thread = Thread::new(project);
    thread.push_user("hello");
    assert_eq!(thread.status, ThreadStatus::Running);
    ThreadStore::new(root.path().into()).save(&thread).unwrap();

    let app = load(root.path(), cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.threads()[0].status, ThreadStatus::Idle)
    });
}
