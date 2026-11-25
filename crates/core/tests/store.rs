use std::fs;
use std::path::Path;

use agent_desk_core::store::{ProjectError, ProjectList, ThreadStore};
use agent_desk_core::thread::{Item, Thread, ThreadId};

fn thread_with_items(project: &Path) -> Thread {
    let mut thread = Thread::new(project.to_path_buf());
    thread.push_user("hello");
    thread.items.push(Item::Assistant("hi there".into()));
    thread
}

#[test]
fn save_then_load_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().into());
    let thread = thread_with_items(root.path());
    store.save(&thread).unwrap();
    assert_eq!(store.load_all(), vec![thread]);
}

#[test]
fn save_overwrites_same_id() {
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().into());
    let mut thread = thread_with_items(root.path());
    store.save(&thread).unwrap();
    thread.items.push(Item::Assistant("more".into()));
    store.save(&thread).unwrap();
    assert_eq!(store.load_all(), vec![thread]);
}

#[test]
fn corrupt_thread_file_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().into());
    store.save(&thread_with_items(root.path())).unwrap();
    fs::write(root.path().join("threads/bad.json"), "{").unwrap();
    assert_eq!(store.load_all().len(), 1);
}

#[test]
fn non_json_files_are_ignored() {
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().into());
    let thread = thread_with_items(root.path());
    store.save(&thread).unwrap();
    // A valid thread left behind in a temp file by an interrupted write must not load.
    let mut stray = thread.clone();
    stray.id = ThreadId::new();
    fs::write(
        root.path().join("threads/x.json.tmp"),
        serde_json::to_string(&stray).unwrap(),
    )
    .unwrap();
    assert_eq!(store.load_all(), vec![thread]);
}

#[test]
fn delete_removes_thread() {
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().into());
    let thread = thread_with_items(root.path());
    store.save(&thread).unwrap();
    store.delete(&thread.id).unwrap();
    assert!(store.load_all().is_empty());
}

#[test]
fn thread_id_cannot_escape_threads_dir() {
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().join("data"));
    let mut thread = thread_with_items(root.path());
    thread.id = ThreadId("../../escaped".into());
    assert!(store.save(&thread).is_err());
    assert!(store.delete(&thread.id).is_err());
    assert!(!root.path().join("escaped.json").exists());
}

#[cfg(unix)]
#[test]
fn saved_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let store = ThreadStore::new(root.path().into());
    let thread = thread_with_items(root.path());
    store.save(&thread).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root.path().join("threads")), 0o700);
    assert_eq!(
        mode(&root.path().join(format!("threads/{}.json", thread.id.0))),
        0o600
    );
}

#[test]
fn project_add_persists() {
    let root = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("my-repo");
    fs::create_dir(&project).unwrap();

    ProjectList::load(root.path().into()).add(&project).unwrap();

    let reloaded = ProjectList::load(root.path().into());
    assert_eq!(reloaded.list().len(), 1);
    assert_eq!(reloaded.list()[0].name, "my-repo");
}

#[test]
fn project_add_twice_is_already_added() {
    let root = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut projects = ProjectList::load(root.path().into());
    projects.add(dir.path()).unwrap();
    assert!(matches!(
        projects.add(&dir.path().join(".")),
        Err(ProjectError::AlreadyAdded(_))
    ));
}

#[test]
fn project_add_file_is_not_a_directory() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.txt");
    fs::write(&file, "x").unwrap();
    let mut projects = ProjectList::load(root.path().into());
    let error = projects.add(&file).unwrap_err();
    assert!(matches!(error, ProjectError::NotADirectory(_)));
    assert!(error.to_string().contains("not a directory"), "{error}");
}

#[test]
fn project_git_detection() {
    let root = tempfile::tempdir().unwrap();
    let git = tempfile::tempdir().unwrap();
    fs::create_dir(git.path().join(".git")).unwrap();
    let plain = tempfile::tempdir().unwrap();

    let mut projects = ProjectList::load(root.path().into());
    assert!(projects.add(git.path()).unwrap().is_git);
    assert!(!projects.add(plain.path()).unwrap().is_git);
}

#[test]
fn project_remove_persists() {
    let root = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut projects = ProjectList::load(root.path().into());
    projects.add(dir.path()).unwrap();
    projects.remove(dir.path()).unwrap();
    assert!(ProjectList::load(root.path().into()).list().is_empty());
}

#[test]
fn corrupt_projects_file_loads_empty() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("projects.json"), "[{").unwrap();
    assert!(ProjectList::load(root.path().into()).list().is_empty());
}
