//! The application model: projects, threads, the selection, live agent
//! sessions, and preferences.
//!
//! `AppState` is the single owner of this data. Fields are private so every
//! change goes through a method that saves what changed and notifies
//! observers; views only read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use agent_desk_core::claude::{
    AgentEvent, ClaudeConfig, ClaudeSession, PermissionSetup, SessionError, SessionEvent,
    is_valid_session_id,
};
use agent_desk_core::detect::{AgentStatus, detect_claude};
use agent_desk_core::settings::{Appearance, Settings};
use agent_desk_core::store::{Project, ProjectList, ThreadStore};
use agent_desk_core::thread::{Thread, ThreadId, ThreadStatus};
use gpui_kit::Context;

/// The binary name used when detection found nothing and no override is set;
/// starting it then fails with a "not found" hint rather than running
/// something unexpected.
const DEFAULT_CLAUDE_BINARY: &str = "claude";

/// A running Claude process for one thread.
struct LiveSession {
    /// Shared with an in-flight `send`; the process is killed when the last
    /// handle drops.
    session: Rc<ClaudeSession>,
    /// Identifies this process's events, so a stopped process's late events
    /// are not applied to its replacement.
    generation: u64,
}

pub struct AppState {
    projects: ProjectList,
    threads: Vec<Thread>,
    selected: Option<ThreadId>,
    claude: AgentStatus,
    last_error: Option<String>,
    settings: Settings,
    sessions: HashMap<ThreadId, LiveSession>,
    next_generation: u64,
    store: ThreadStore,
    root: PathBuf,
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
            projects: ProjectList::load(root.clone()),
            threads,
            selected: None,
            claude: detect_claude(claude_override.as_deref()),
            last_error: None,
            settings: Settings::load(&root),
            sessions: HashMap::new(),
            next_generation: 0,
            store,
            root,
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

    pub fn appearance(&self) -> Appearance {
        self.settings.appearance
    }

    /// The most recent failed action, cleared by the next successful one.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Whether the thread has a running Claude process.
    pub fn is_live(&self, id: &ThreadId) -> bool {
        self.sessions.contains_key(id)
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
        self.threads.push(thread);
        self.save_thread(self.threads.len() - 1);
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

    /// Records the message on the thread and sends it to the thread's Claude
    /// process, starting one (resuming the saved session) if none is live.
    ///
    /// The message is kept even when sending fails; the thread's status then
    /// says why.
    pub fn send_message(&mut self, id: &ThreadId, text: String, cx: &mut Context<Self>) {
        let Some(ix) = self.thread_ix(id) else {
            return;
        };
        self.threads[ix].push_user(&text);

        let project = self.threads[ix].project.clone();
        let session = if !project.is_dir() {
            Err(format!("Project folder not found: {}", project.display()))
        } else if let Some(live) = self.sessions.get(id) {
            Ok(live.session.clone())
        } else {
            self.start_session(ix, cx)
        };
        match session {
            Ok(session) => {
                cx.spawn(async move |_, _| {
                    // A process that died is reported by its Exited event.
                    if let Err(error) = session.send(&text).await {
                        log::warn!("sending to Claude: {error}");
                    }
                })
                .detach();
            }
            Err(reason) => self.threads[ix].status = ThreadStatus::Failed(reason),
        }
        self.save_thread(ix);
        cx.notify();
    }

    /// Stops the thread's Claude process. A turn in progress ends as "Stopped".
    pub fn stop(&mut self, id: &ThreadId, cx: &mut Context<Self>) {
        // Dropping the last handle kills the process; its late Exited event
        // no longer matches a live generation and is ignored.
        if self.sessions.remove(id).is_none() {
            return;
        }
        if let Some(ix) = self.thread_ix(id) {
            if self.threads[ix].status == ThreadStatus::Running {
                self.threads[ix].status = ThreadStatus::Failed("Stopped".into());
            }
            self.save_thread(ix);
        }
        cx.notify();
    }

    pub fn recheck_claude(&mut self, cx: &mut Context<Self>) {
        self.claude = detect_claude(self.claude_override.as_deref());
        cx.notify();
    }

    pub fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if self.settings.appearance == appearance {
            return;
        }
        self.settings.appearance = appearance;
        if let Err(error) = self.settings.save(&self.root) {
            log::warn!("saving settings: {error}");
            self.last_error = Some(format!("Couldn’t save the appearance setting: {error}"));
        }
        cx.notify();
    }

    fn start_session(
        &mut self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Result<Rc<ClaudeSession>, String> {
        let thread = &self.threads[ix];
        let resume = thread.session_id.clone().filter(|session_id| {
            let is_valid = is_valid_session_id(session_id);
            if !is_valid {
                log::warn!(
                    "not resuming thread {}: unsafe saved session id",
                    thread.id.0
                );
            }
            is_valid
        });
        let config = ClaudeConfig {
            binary: self.claude_binary(),
            cwd: thread.project.clone(),
            resume,
            permission: PermissionSetup::FullAccess,
        };
        let (session, events) = ClaudeSession::start(&config).map_err(|error| match error {
            SessionError::NotFound(_) => {
                "Claude Code not found — install it from the banner".to_owned()
            }
            SessionError::CwdMissing(path) => {
                format!("Project folder not found: {}", path.display())
            }
            SessionError::Io(error) => format!("Couldn’t start Claude Code: {error}"),
        })?;

        let id = thread.id.clone();
        let generation = self.next_generation;
        self.next_generation += 1;
        let session = Rc::new(session);
        self.sessions.insert(
            id.clone(),
            LiveSession {
                session: session.clone(),
                generation,
            },
        );

        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let applied = this.update(cx, |this, cx| {
                    this.on_session_event(&id, generation, event, cx)
                });
                if applied.is_err() {
                    break; // The app state is gone.
                }
            }
        })
        .detach();
        Ok(session)
    }

    fn on_session_event(
        &mut self,
        id: &ThreadId,
        generation: u64,
        event: SessionEvent,
        cx: &mut Context<Self>,
    ) {
        let is_current = self
            .sessions
            .get(id)
            .is_some_and(|live| live.generation == generation);
        let Some(ix) = self.thread_ix(id).filter(|_| is_current) else {
            return;
        };
        match event {
            SessionEvent::Agent(event) => {
                let is_turn_done = matches!(event, AgentEvent::TurnDone { .. });
                self.threads[ix].apply(&event);
                if is_turn_done {
                    self.save_thread(ix);
                }
            }
            SessionEvent::Exited { code, stderr_tail } => {
                self.threads[ix].on_exit(code, &stderr_tail);
                self.sessions.remove(id);
                self.save_thread(ix);
            }
        }
        cx.notify();
    }

    /// The detected binary, else the override, else the bare name.
    fn claude_binary(&self) -> PathBuf {
        match &self.claude {
            AgentStatus::Installed { path, .. } => path.clone(),
            _ => self
                .claude_override
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_CLAUDE_BINARY)),
        }
    }

    fn thread_ix(&self, id: &ThreadId) -> Option<usize> {
        self.threads.iter().position(|thread| &thread.id == id)
    }

    fn save_thread(&mut self, ix: usize) {
        if let Err(error) = self.store.save(&self.threads[ix]) {
            let thread = &self.threads[ix];
            log::warn!("saving thread {}: {error}", thread.id.0);
            self.last_error = Some(format!("Couldn’t save “{}”: {error}", thread.title));
        }
    }
}
