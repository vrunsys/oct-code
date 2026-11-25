//! The window's root view: the project sidebar beside the thread pane.
//!
//! `WorkspaceView` owns window-level workflow (adding projects, creating
//! threads, installing Claude) and composes the child views. Domain data
//! lives in [`AppState`]; this view keeps only transient UI state.

use std::path::Path;

use agent_desk_core::detect::{AgentStatus, current_os, install_command, run_install};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, rems,
};

use crate::actions::{AddProject, NewThread, WORKSPACE_CONTEXT};
use crate::app_state::AppState;
use crate::appearance::Appearance;
use crate::composer::Composer;
use crate::layout::content_column;
use crate::thread_view::ThreadView;

/// The installer log keeps only its most recent lines.
const INSTALL_LOG_MAX_LINES: usize = 500;

#[derive(Default)]
struct InstallState {
    is_running: bool,
    log: Vec<SharedString>,
    is_log_open: bool,
    /// The installer's exit code from the last run that finished.
    last_exit: Option<Option<i32>>,
}

pub struct WorkspaceView {
    app: Entity<AppState>,
    thread_view: Entity<ThreadView>,
    composer: Entity<Composer>,
    install: InstallState,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceView {
    pub fn new(
        app: Entity<AppState>,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let thread_view = cx.new(|cx| ThreadView::new(app.clone(), cx));
        let composer = cx.new(|cx| Composer::new(app.clone(), window, cx));
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        appearance.apply(window, cx);
        let subscriptions = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            // Follow the OS between light and dark while the app is open.
            cx.observe_window_appearance(window, move |_, window, cx| appearance.apply(window, cx)),
        ];
        Self {
            app,
            thread_view,
            composer,
            install: InstallState::default(),
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    log::warn!("folder picker failed: {error}");
                    let _ = this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error("Couldn’t open the folder picker."),
                            cx,
                        );
                    });
                    None
                }
            };
            if let Some(path) = path {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.add_project_at(&path, window, cx)
                });
            }
        })
        .detach();
    }

    fn add_project_at(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| app.add_project(path, cx));
        if let Some(error) = self.app.read(cx).last_error() {
            window.push_notification(Notification::error(error.to_owned()), cx);
        }
    }

    /// Starts a thread in `project` and puts the cursor in its composer.
    fn new_thread(&mut self, project: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            app.new_thread(project, cx);
        });
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
    }

    /// The keyboard path: a new thread in the current thread's project, or
    /// the first project, or the folder picker when there are none.
    fn new_thread_in_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let project = app
            .selected_thread()
            .map(|thread| thread.project.clone())
            .or_else(|| app.projects().first().map(|project| project.path.clone()));
        match project {
            Some(project) => self.new_thread(&project, window, cx),
            None => self.add_project(window, cx),
        }
    }

    fn select_thread(
        &mut self,
        id: &agent_desk_core::thread::ThreadId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app.update(cx, |app, cx| app.select(id, cx));
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
    }

    /// Asks before running the installer: it downloads and runs a script, so
    /// the dialog shows the exact command.
    fn confirm_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (program, args) = install_command(current_os());
        // Quote arguments with spaces so the dialog shows the exact invocation.
        let quoted: Vec<String> = args
            .iter()
            .map(|arg| {
                if arg.contains(' ') {
                    format!("\"{arg}\"")
                } else {
                    (*arg).to_owned()
                }
            })
            .collect();
        let command = format!("{program} {}", quoted.join(" "));
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            dialog
                .confirm()
                .title("Install Claude Code?")
                .description(
                    v_flex()
                        .gap_2()
                        .child("Agent Desk will run the official installer from claude.ai:")
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().muted)
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(command.clone()),
                        ),
                )
                .ok_text("Install")
                .on_ok(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.start_install(window, cx));
                    true
                })
        });
    }

    fn start_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.install.is_running {
            return;
        }
        self.install = InstallState {
            is_running: true,
            is_log_open: true,
            ..Default::default()
        };
        cx.notify();

        let (lines, received) = async_channel::unbounded::<String>();
        let installer = cx.background_spawn(run_install(current_os(), move |line| {
            let _ = lines.try_send(line);
        }));
        cx.spawn_in(window, async move |this, cx| {
            // Ends when the installer finishes and drops its sender.
            while let Ok(line) = received.recv().await {
                let _ = this.update(cx, |this, cx| {
                    let log = &mut this.install.log;
                    log.push(line.into());
                    if log.len() > INSTALL_LOG_MAX_LINES {
                        log.drain(..log.len() - INSTALL_LOG_MAX_LINES);
                    }
                    cx.notify();
                });
            }
            let exit = installer.await;
            let _ = this.update_in(cx, |this, window, cx| this.finish_install(exit, window, cx));
        })
        .detach();
    }

    fn finish_install(
        &mut self,
        exit: std::io::Result<Option<i32>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.install.is_running = false;
        match exit {
            Ok(code) => {
                self.install.last_exit = Some(code);
                if code == Some(0) {
                    self.app.update(cx, |app, cx| app.recheck_claude(cx));
                } else {
                    window.push_notification(
                        Notification::error(
                            "The installer didn’t finish. Check the log for details.",
                        ),
                        cx,
                    );
                }
            }
            Err(error) => {
                self.install
                    .log
                    .push(format!("Couldn’t start the installer: {error}").into());
                window.push_notification(Notification::error("Couldn’t start the installer."), cx);
            }
        }
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let app = self.app.read(cx);
        let selected = app.selected().cloned();

        let projects = app.projects().iter().map(|project| {
            let threads = app.threads_for(&project.path).into_iter().map(|thread| {
                let id = thread.id.clone();
                let this = this.clone();
                let is_selected = selected.as_ref() == Some(&thread.id);
                SidebarMenuItem::new(thread.title.clone())
                    .active(is_selected)
                    .on_click(move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| this.select_thread(&id, window, cx));
                    })
            });
            let path = project.path.clone();
            let this = this.clone();
            SidebarMenuItem::new(project.name.clone())
                .icon(IconName::Folder)
                .default_open(true)
                .suffix(move |_, _| {
                    let path = path.clone();
                    let this = this.clone();
                    Button::new(new_thread_button_id(&path))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("New thread")
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |this, cx| this.new_thread(&path, window, cx));
                        })
                })
                .children(threads)
        });
        let projects: Vec<SidebarMenuItem> = projects.collect();
        let add_project = this.clone();

        Sidebar::new("sidebar")
            .header(
                SidebarHeader::new().child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Agent Desk"),
                ),
            )
            .child(
                SidebarGroup::new("Projects")
                    .suffix(move |_, _| {
                        let this = add_project.clone();
                        Button::new("add-project")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("Add project…")
                            .on_click(move |_, window, cx| {
                                let _ = this.update(cx, |this, cx| this.add_project(window, cx));
                            })
                    })
                    .child(SidebarMenu::new().children(projects)),
            )
            .footer(SidebarFooter::new().child(self.render_claude_status(cx)))
    }

    fn render_claude_status(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, color, label): (IconName, _, SharedString) = match self.app.read(cx).claude() {
            AgentStatus::Installed { version, .. } => (
                IconName::Check,
                theme.success,
                format!("Claude Code {version}").into(),
            ),
            AgentStatus::Missing => (
                IconName::TriangleAlert,
                theme.warning,
                "Claude Code not found".into(),
            ),
            AgentStatus::Broken { .. } => (
                IconName::TriangleAlert,
                theme.warning,
                "Claude Code unavailable".into(),
            ),
        };
        h_flex()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).xsmall().text_color(color))
            .child(label)
    }

    fn render_claude_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let detail = match self.app.read(cx).claude() {
            AgentStatus::Installed { .. } => return None,
            AgentStatus::Missing => None,
            AgentStatus::Broken { path, error } => Some(format!("{}: {error}", path.display())),
        };
        let theme = cx.theme();
        let install = &self.install;
        let finished_but_missing = install.last_exit == Some(Some(0));

        let message = if finished_but_missing {
            "Claude Code was installed, but Agent Desk can’t find it yet. Restart Agent Desk, or set AGENT_DESK_CLAUDE_BIN.".to_owned()
        } else {
            detail.unwrap_or_else(|| "Install it to start working with an agent.".to_owned())
        };

        let row = h_flex()
            .gap_3()
            .px_4()
            .py_2()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .small()
                    .text_color(theme.warning),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Claude Code not found"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(message),
                    ),
            )
            .when(!install.log.is_empty(), |this| {
                this.child(
                    Button::new("install-log")
                        .ghost()
                        .small()
                        .label(if install.is_log_open {
                            "Hide log"
                        } else {
                            "Show log"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.install.is_log_open = !this.install.is_log_open;
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("recheck-claude")
                    .ghost()
                    .small()
                    .label("Recheck")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.app.update(cx, |app, cx| app.recheck_claude(cx));
                    })),
            )
            .child(
                Button::new("install-claude")
                    .outline()
                    .small()
                    .label("Install…")
                    .loading(install.is_running)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_install(window, cx))),
            );

        let log = install.is_log_open.then(|| {
            div()
                .id("install-log-lines")
                .mx_4()
                .mb_2()
                .px_3()
                .py_2()
                .max_h(rems(12.))
                .overflow_y_scroll()
                .rounded(theme.radius)
                .bg(theme.muted)
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .children(install.log.iter().cloned())
        });

        Some(
            v_flex()
                .border_b_1()
                .border_color(theme.border)
                .child(row)
                .children(log)
                .into_any_element(),
        )
    }

    fn render_thread_header(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let app = self.app.read(cx);
        let thread = app.selected_thread()?;
        let project_name = app
            .projects()
            .iter()
            .find(|project| project.path == thread.project)
            .map(|project| project.name.clone())
            .unwrap_or_else(|| thread.project.display().to_string());
        let theme = cx.theme();
        Some(
            div().border_b_1().border_color(theme.border).py_3().child(
                content_column()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .min_w_0()
                    .child(div().text_color(theme.muted_foreground).child(project_name))
                    .child(div().text_color(theme.muted_foreground).child("›"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(thread.title.clone()),
                    ),
            ),
        )
    }

    fn render_main(&self, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        if app.selected_thread().is_some() {
            return v_flex()
                .size_full()
                .min_w_0()
                .children(self.render_thread_header(cx))
                .child(div().flex_1().min_h_0().child(self.thread_view.clone()))
                .child(self.composer.clone())
                .into_any_element();
        }
        if app.projects().is_empty() {
            return empty_state(
                "No projects yet",
                "Add a folder to start a thread with Claude Code.",
                Some(
                    Button::new("add-project-empty")
                        .label("Add project…")
                        .icon(IconName::Folder)
                        .on_click(cx.listener(|this, _, window, cx| this.add_project(window, cx))),
                ),
                cx,
            );
        }
        empty_state(
            "No thread selected",
            "Pick a thread in the sidebar, or start one with the + next to a project.",
            None::<Button>,
            cx,
        )
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("workspace")
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &AddProject, window, cx| this.add_project(window, cx)))
            .on_action(
                cx.listener(|this, _: &NewThread, window, cx| {
                    this.new_thread_in_context(window, cx)
                }),
            )
            .items_stretch()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_sidebar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .children(self.render_claude_banner(cx))
                    .child(div().flex_1().min_h_0().child(self.render_main(cx))),
            )
    }
}

/// A stable id for a project's "New thread" button, derived from its path.
pub fn new_thread_button_id(project: &Path) -> ElementId {
    ElementId::Name(format!("new-thread:{}", project.display()).into())
}

fn empty_state(
    title: &'static str,
    description: &'static str,
    action: Option<impl IntoElement>,
    cx: &Context<WorkspaceView>,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(description),
        )
        .children(action.map(|action| div().pt_4().child(action)))
        .into_any_element()
}
