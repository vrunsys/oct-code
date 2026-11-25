//! The window's root view: the project sidebar beside the thread pane.
//!
//! `WorkspaceView` owns window-level workflow (adding projects, creating and
//! stopping threads, choosing the appearance) and composes the child views.
//! Domain data lives in [`AppState`]; this view keeps only transient UI state.

use std::path::Path;

use agent_desk_core::detect::AgentStatus;
use agent_desk_core::settings::Appearance;
use agent_desk_core::thread::{Thread, ThreadId, ThreadStatus};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};

use crate::actions::{AddProject, NewThread, StopThread, ToggleAppearance, WORKSPACE_CONTEXT};
use crate::app_state::AppState;
use crate::appearance::apply_appearance;
use crate::claude_banner::ClaudeBanner;
use crate::composer::Composer;
use crate::layout::content_column;
use crate::thread_view::ThreadView;

pub struct WorkspaceView {
    app: Entity<AppState>,
    thread_view: Entity<ThreadView>,
    composer: Entity<Composer>,
    claude_banner: Entity<ClaudeBanner>,
    /// A one-run override from the environment; cleared once the user
    /// chooses an appearance in the app.
    appearance_override: Option<Appearance>,
    /// The appearance last applied to the window, to apply only on change.
    applied_appearance: Appearance,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceView {
    pub fn new(
        app: Entity<AppState>,
        appearance_override: Option<Appearance>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let thread_view = cx.new(|cx| ThreadView::new(app.clone(), cx));
        let composer = cx.new(|cx| Composer::new(app.clone(), window, cx));
        let claude_banner = cx.new(|cx| ClaudeBanner::new(app.clone(), cx));
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        // Sessions and turns added outside Agent Desk, now and while it runs.
        app.update(cx, |app, cx| app.watch_history(cx));

        let applied_appearance = appearance_override.unwrap_or(app.read(cx).appearance());
        apply_appearance(applied_appearance, window, cx);
        let subscriptions = vec![
            cx.observe_in(&app, window, |this, _, window, cx| {
                this.apply_appearance_if_changed(window, cx);
                cx.notify();
            }),
            // Follow the OS between light and dark while the app is open.
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.applied_appearance == Appearance::System {
                    apply_appearance(Appearance::System, window, cx);
                }
            }),
        ];
        Self {
            app,
            thread_view,
            composer,
            claude_banner,
            appearance_override,
            applied_appearance,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    fn appearance(&self, cx: &Context<Self>) -> Appearance {
        self.appearance_override
            .unwrap_or(self.app.read(cx).appearance())
    }

    fn apply_appearance_if_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let appearance = self.appearance(cx);
        if appearance != self.applied_appearance {
            self.applied_appearance = appearance;
            apply_appearance(appearance, window, cx);
        }
    }

    /// The user's choice wins over the environment override and is saved.
    fn choose_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance_override = None;
        self.app
            .update(cx, |app, cx| app.set_appearance(appearance, cx));
        self.apply_appearance_if_changed(window, cx);
        cx.notify();
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

    fn select_thread(&mut self, id: &ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| app.select(id, cx));
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
    }

    fn stop_selected_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.app.read(cx).selected().cloned() {
            self.app.update(cx, |app, cx| app.stop(&id, cx));
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let app = self.app.read(cx);
        let selected = app.selected().cloned();

        let projects: Vec<SidebarMenuItem> = app
            .projects()
            .iter()
            .map(|project| {
                let threads = app.threads_for(&project.path).into_iter().map(|thread| {
                    let id = thread.id.clone();
                    let this = this.clone();
                    SidebarMenuItem::new(thread.title.clone())
                        .active(selected.as_ref() == Some(&thread.id))
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
                                let _ =
                                    this.update(cx, |this, cx| this.new_thread(&path, window, cx));
                            })
                    })
                    .children(threads)
            })
            .collect();

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
                        let this = this.clone();
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
            .footer(
                // A plain row with SidebarFooter's padding: SidebarFooter is styled
                // as a clickable trigger, and this footer is not one.
                h_flex()
                    .w_full()
                    .gap_2()
                    .p_2()
                    .justify_between()
                    .child(self.render_claude_status(cx))
                    .child(self.render_appearance_toggle(cx)),
            )
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
            .min_w_0()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).xsmall().text_color(color))
            .child(div().truncate().child(label))
    }

    /// One click switches between light and dark. The icon shows the
    /// current mode; the tooltip names the result.
    fn render_appearance_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().mode.is_dark();
        Button::new("appearance")
            .ghost()
            .xsmall()
            .icon(if is_dark {
                IconName::Moon
            } else {
                IconName::Sun
            })
            .accessibility_label("Toggle light or dark mode")
            .tooltip(if is_dark {
                "Switch to light mode"
            } else {
                "Switch to dark mode"
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_appearance(window, cx)))
    }

    /// Switches to the opposite of what is showing; from then on the choice
    /// is saved and no longer follows the system.
    fn toggle_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if cx.theme().mode.is_dark() {
            Appearance::Light
        } else {
            Appearance::Dark
        };
        self.choose_appearance(next, window, cx);
    }

    fn render_thread_header(&self, thread: &Thread, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        let project_name = app
            .projects()
            .iter()
            .find(|project| project.path == thread.project)
            .map(|project| project.name.clone())
            .unwrap_or_else(|| thread.project.display().to_string());
        let is_running = thread.status == ThreadStatus::Running;
        let is_stoppable = is_running && app.is_live(&thread.id);
        let theme = cx.theme();

        div()
            .border_b_1()
            .border_color(theme.border)
            .py_2()
            .child(
                content_column()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h_7()
                    .text_sm()
                    .child(div().text_color(theme.muted_foreground).child(project_name))
                    .child(div().text_color(theme.muted_foreground).child("›"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(thread.title.clone()),
                    )
                    .when(is_running, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(Spinner::new().xsmall())
                                .child("Working"),
                        )
                    })
                    .when(is_stoppable, |this| {
                        this.child(
                            Button::new("stop-thread")
                                .ghost()
                                .small()
                                .label("Stop")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.stop_selected_thread(cx)),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Why the last turn failed, next to where the user retries.
    fn render_failure(reason: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        content_column()
            .pb_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_sm()
                    .child(
                        div().pt_0p5().child(
                            Icon::new(IconName::CircleX)
                                .xsmall()
                                .text_color(theme.danger),
                        ),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().child(reason.to_owned()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Send a message to try again."),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_main(&self, cx: &mut Context<Self>) -> AnyElement {
        let app = self.app.read(cx);
        if let Some(thread) = app.selected_thread().cloned() {
            let header = self.render_thread_header(&thread, cx);
            let failure = match &thread.status {
                ThreadStatus::Failed(reason) => Some(Self::render_failure(reason, cx)),
                _ => None,
            };
            return v_flex()
                .size_full()
                .min_w_0()
                .child(header)
                .child(div().flex_1().min_h_0().child(self.thread_view.clone()))
                .children(failure)
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
            .on_action(cx.listener(|this, _: &StopThread, _, cx| this.stop_selected_thread(cx)))
            .on_action(cx.listener(|this, _: &ToggleAppearance, window, cx| {
                this.toggle_appearance(window, cx)
            }))
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
                    .child(self.claude_banner.clone())
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
