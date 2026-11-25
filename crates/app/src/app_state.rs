//! The application model: projects, threads, the selection, and whether
//! Claude Code is available.
//!
//! `AppState` is the single owner of this data. Fields are private so every
//! change goes through a method that saves what changed and notifies
//! observers; views only read.

use std::path::{Path, PathBuf};

use agent_desk_core::detect::{AgentStatus, detect_claude};
use agent_desk_core::store::{Project, ProjectList, ThreadStore};
use agent_desk_core::thread::{Thread, ThreadId, ThreadStatus};
use gpui_kit::Context;

pub struct AppState {
    projects: ProjectList,
    threads: Vec<Thread>,
    selected: Option<ThreadId>,
    claude: AgentStatus,
    last_error: Option<String>,
    store: ThreadStore,
    claude_override: Option<PathBuf>,
}

impl AppState {
    /// Loads everything under `root`. Never fails: unreadable data is skipped
    /// by the store and reported in the log.
    pub fn load(root: PathBuf, claude_override: Option<PathBuf>) -> Self {
        let store = ThreadStore::new(root.clone());
        let mut threads = store.load_all();
        for thread in &mut threads {
            // No agent process survives a restart.
            if thread.status == ThreadStatus::Running {
                thread.status = ThreadStatus::Idle;
            }
        }
        Self {
            projects: ProjectList::load(root),
            threads,
            selected: None,
            claude: detect_claude(claude_override.as_deref()),
            last_error: None,
            store,
            claude_override,
        }
    }

    pub fn projects(&self) -> &[Project] {
        self.projects.list()
    }

    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    pub fn selected(&self) -> Option<&ThreadId> {
        self.selected.as_ref()
    }

    pub fn claude(&self) -> &AgentStatus {
        &self.claude
    }

    /// The most recent failed action, cleared by the next successful one.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn selected_thread(&self) -> Option<&Thread> {
        let id = self.selected.as_ref()?;
        self.threads.iter().find(|thread| &thread.id == id)
    }

    /// The project's threads, newest first. Threads created in the same
    /// millisecond keep creation order: later-added comes first.
    pub fn threads_for(&self, project: &Path) -> Vec<&Thread> {
        let mut threads: Vec<&Thread> = self
            .threads
            .iter()
            .rev()
            .filter(|thread| thread.project == project)
            .collect();
        // A stable sort, so equal timestamps keep the reversed insertion order.
        threads.sort_by_key(|thread| std::cmp::Reverse(thread.created_at));
        threads
    }

    pub fn add_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.last_error = self.projects.add(path).err().map(|error| error.to_string());
        cx.notify();
    }

    /// Creates an empty thread in `project`, selects it and saves it.
    pub fn new_thread(&mut self, project: &Path, cx: &mut Context<Self>) -> ThreadId {
        let thread = Thread::new(project.to_path_buf());
        let id = thread.id.clone();
        self.save(&thread);
        self.threads.push(thread);
        self.selected = Some(id.clone());
        cx.notify();
        id
    }

    pub fn select(&mut self, id: &ThreadId, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(id) && self.threads.iter().any(|thread| &thread.id == id)
        {
            self.selected = Some(id.clone());
            cx.notify();
        }
    }

    /// Records the user's message on the thread and saves it.
    pub fn send_message(&mut self, id: &ThreadId, text: String, cx: &mut Context<Self>) {
        let Some(ix) = self.threads.iter().position(|thread| &thread.id == id) else {
            return;
        };
        self.threads[ix].push_user(&text);
        let thread = self.threads[ix].clone();
        self.save(&thread);
        cx.notify();
    }

    pub fn recheck_claude(&mut self, cx: &mut Context<Self>) {
        self.claude = detect_claude(self.claude_override.as_deref());
        cx.notify();
    }

    fn save(&mut self, thread: &Thread) {
        if let Err(error) = self.store.save(thread) {
            log::warn!("saving thread {}: {error}", thread.id.0);
            self.last_error = Some(format!("Couldn’t save “{}”: {error}", thread.title));
        }
    }
}
