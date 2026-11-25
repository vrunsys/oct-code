//! The application model: projects, threads, the selection, live agent
//! sessions, and preferences.
//!
//! `AppState` is the single owner of this data. Fields are private so every
//! change goes through a method that saves what changed and notifies
//! observers; views only read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use agent_desk_core::claude::catalog::{self, ModelCatalog};
use agent_desk_core::claude::history::{self, HistoryEntry, HistorySession, HistoryWatcher};
use agent_desk_core::claude::{
    AgentEvent, ClaudeConfig, ClaudeSession, PermissionSetup, SessionError, SessionEvent,
    is_plain_token, is_valid_session_id,
};
use agent_desk_core::detect::{AgentStatus, detect_claude};
use agent_desk_core::settings::{Appearance, Settings};
use agent_desk_core::store::{Project, ProjectList, ThreadStore};
use agent_desk_core::thread::{Thread, ThreadId, ThreadStatus};
use gpui_kit::{AppContext as _, Context, Task};

/// The binary name used when detection found nothing and no override is set;
/// starting it then fails with a "not found" hint rather than running
/// something unexpected.
const DEFAULT_CLAUDE_BINARY: &str = "claude";

/// How often Claude Code's history is checked for turns and sessions added
/// outside Agent Desk. A check only reads the files that changed.
const HISTORY_POLL_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Claude Code's config directory, read for a project's past sessions.
    history_home: Option<PathBuf>,
    /// The models and reasoning levels Claude Code offers this account.
    catalog: ModelCatalog,
    /// Follows Claude Code's history while the app runs; dropped with it.
    history_watch: Option<Task<()>>,
    /// Counts the times a thread's items were replaced rather than added to.
    rewrites: u64,
}

impl AppState {
    /// Loads everything under `root`. Never fails: unreadable data is skipped
    /// by the store and reported in the log.
    pub fn load(root: PathBuf, claude_override: Option<PathBuf>) -> Self {
        Self::load_with_history(root, claude_override, history::claude_home())
    }

    /// [`AppState::load`] with an explicit Claude Code config directory for
    /// past sessions; `None` imports none.
    pub fn load_with_history(
        root: PathBuf,
        claude_override: Option<PathBuf>,
        history_home: Option<PathBuf>,
    ) -> Self {
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
            catalog: history_home
                .as_deref()
                .and_then(catalog::load_catalog)
                .unwrap_or_else(ModelCatalog::fallback),
            history_home,
            history_watch: None,
            rewrites: 0,
        }
    }

    /// The models and reasoning levels on offer, from Claude Code's own
    /// catalog, or its CLI aliases when that is unavailable.
    pub fn catalog(&self) -> &ModelCatalog {
        &self.catalog
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
    /// Changes whenever a thread's items are replaced rather than added to,
    /// so a view that caches item layout knows to start over.
    pub fn rewrites(&self) -> u64 {
        self.rewrites
    }

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

    /// Adds the folder and shows its existing Claude Code sessions as threads.
    pub fn add_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        match self.projects.add(path) {
            Ok(project) => {
                self.last_error = None;
                self.import_history(project.path, cx);
            }
            Err(error) => self.last_error = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Shows every project's Claude Code sessions, then keeps following
    /// them: a turn or session added in the terminal or the VS Code
    /// extension appears within [`HISTORY_POLL_INTERVAL`]. Calling it again
    /// while it runs does nothing.
    pub fn watch_history(&mut self, cx: &mut Context<Self>) {
        let Some(home) = self.history_home.clone() else {
            return;
        };
        if self.history_watch.is_some() {
            return;
        }
        self.history_watch = Some(cx.spawn(async move |this, cx| {
            let mut watchers: HashMap<PathBuf, HistoryWatcher> = HashMap::new();
            loop {
                let Ok(projects) = this.read_with(cx, |this, _| {
                    this.projects()
                        .iter()
                        .map(|project| project.path.clone())
                        .collect::<Vec<_>>()
                }) else {
                    return;
                };
                watchers.retain(|project, _| projects.contains(project));
                for project in projects {
                    watchers
                        .entry(project)
                        .or_insert_with_key(|project| HistoryWatcher::new(&home, project));
                }
                let (polled, changes) = cx
                    .background_spawn(async move {
                        let changes: Vec<_> = watchers
                            .iter_mut()
                            .map(|(project, watcher)| (project.clone(), watcher.poll()))
                            .filter(|(_, sessions)| !sessions.is_empty())
                            .collect();
                        (watchers, changes)
                    })
                    .await;
                watchers = polled;
                let applied = this.update(cx, |this, cx| {
                    for (project, sessions) in changes {
                        this.merge_history_sessions(&project, sessions, cx);
                    }
                });
                if applied.is_err() {
                    return;
                }
                cx.background_executor().timer(HISTORY_POLL_INTERVAL).await;
            }
        }));
    }

    /// Reads the project's Claude Code history off the UI thread, then adds
    /// each session that is not already a thread.
    fn import_history(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        let Some(home) = self.history_home.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let scan_project = project.clone();
            let sessions = cx
                .background_spawn(
                    async move { history::sessions_for_project(&home, &scan_project) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.merge_history_sessions(&project, sessions, cx)
            });
        })
        .detach();
    }

    /// Adds sessions that are not threads yet and brings the others up to
    /// date.
    fn merge_history_sessions(
        &mut self,
        project: &Path,
        sessions: Vec<HistorySession>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for session in sessions {
            let known = self.threads.iter().position(|thread| {
                thread.session_id.as_deref() == Some(session.session_id.as_str())
            });
            match known {
                Some(ix) => changed |= self.refresh_from_history(ix, session),
                None => {
                    self.threads.push(thread_from_history(project, session));
                    self.save_thread(self.threads.len() - 1);
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Gives a thread the turns its session gained outside Agent Desk. A
    /// thread with a turn running here is left alone: its own stream is
    /// newer than the file Claude is still writing.
    fn refresh_from_history(&mut self, ix: usize, session: HistorySession) -> bool {
        let thread = &self.threads[ix];
        if self.is_live(&thread.id) || thread.status == ThreadStatus::Running {
            return false;
        }
        let fresh = thread_from_history(&thread.project, session);
        if fresh.items == thread.items {
            return false;
        }
        if !fresh.items.starts_with(&thread.items) {
            self.rewrites += 1;
        }
        let thread = &mut self.threads[ix];
        thread.items = fresh.items;
        thread.title = fresh.title;
        // Newer turns supersede how the last one here ended.
        thread.status = fresh.status;
        self.save_thread(ix);
        true
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

    /// Sets the thread's model; `None` uses Claude's default. Only catalog
    /// models are kept. A reasoning level the new model lacks is cleared, and
    /// a live session ends so the next message restarts with the new model
    /// and resumes the same conversation.
    pub fn set_model(&mut self, id: &ThreadId, model: Option<String>, cx: &mut Context<Self>) {
        let Some(ix) = self.thread_ix(id) else {
            return;
        };
        let model = model.filter(|model| self.catalog.model(model).is_some());
        if self.threads[ix].model == model {
            return;
        }
        if let Some(effort) = &self.threads[ix].effort
            && !self.effort_is_offered(model.as_deref(), effort)
        {
            self.threads[ix].effort = None;
        }
        self.threads[ix].model = model;
        // Dropping the session kills the process; nothing is mid-turn, as the
        // composer disables the picker while a turn runs.
        self.sessions.remove(id);
        self.save_thread(ix);
        cx.notify();
    }

    /// Sets the thread's reasoning level; `None` uses the model's default.
    /// Only levels the thread's model offers are kept, and a live session
    /// ends as for a model change.
    pub fn set_effort(&mut self, id: &ThreadId, effort: Option<String>, cx: &mut Context<Self>) {
        let Some(ix) = self.thread_ix(id) else {
            return;
        };
        let model = self.threads[ix].model.clone();
        let effort = effort.filter(|effort| self.effort_is_offered(model.as_deref(), effort));
        if self.threads[ix].effort == effort {
            return;
        }
        self.threads[ix].effort = effort;
        self.sessions.remove(id);
        self.save_thread(ix);
        cx.notify();
    }

    /// Whether `effort` is a level `model` takes.
    fn effort_is_offered(&self, model: Option<&str>, effort: &str) -> bool {
        self.catalog
            .efforts_for(model)
            .iter()
            .any(|level| level.id == effort)
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
            model: thread.model.clone().filter(|model| is_plain_token(model)),
            effort: thread
                .effort
                .clone()
                .filter(|effort| is_plain_token(effort)),
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

/// A thread holding a past session's conversation, ready to resume it.
fn thread_from_history(project: &Path, session: HistorySession) -> Thread {
    let mut thread = Thread::new(project.to_path_buf());
    for entry in &session.entries {
        match entry {
            HistoryEntry::User(text) => thread.push_user(text),
            HistoryEntry::Agent(event) => thread.apply(event),
        }
    }
    // History has no turn results, and no process is running for it.
    thread.status = ThreadStatus::Idle;
    thread.session_id = Some(session.session_id);
    thread.created_at = session.modified_ms;
    if let Some(title) = session.title.filter(|title| !title.trim().is_empty()) {
        thread.title = title;
    }
    thread
}
