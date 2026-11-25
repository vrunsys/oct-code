//! The "Claude Code not found" banner and the installer it runs.
//!
//! An entity of its own because installing has its own state and lifecycle:
//! a confirmation, a background process, and a streamed log. It renders
//! nothing while Claude is installed.

use agent_desk_core::detect::{AgentStatus, current_os, install_command, run_install};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, rems,
};

use crate::app_state::AppState;

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

pub struct ClaudeBanner {
    app: Entity<AppState>,
    install: InstallState,
    _app_subscription: Subscription,
}

impl ClaudeBanner {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            install: InstallState::default(),
            _app_subscription: subscription,
        }
    }

    /// Asks before running the installer: it downloads and runs a script, so
    /// the dialog shows the exact command.
    fn confirm_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = installer_command_line();
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
                let _ = this.update(cx, |this, cx| this.push_log_line(line.into(), cx));
            }
            let exit = installer.await;
            let _ = this.update_in(cx, |this, window, cx| this.finish_install(exit, window, cx));
        })
        .detach();
    }

    fn push_log_line(&mut self, line: SharedString, cx: &mut Context<Self>) {
        let log = &mut self.install.log;
        log.push(line);
        if log.len() > INSTALL_LOG_MAX_LINES {
            log.drain(..log.len() - INSTALL_LOG_MAX_LINES);
        }
        cx.notify();
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
                self.push_log_line(format!("Couldn’t start the installer: {error}").into(), cx);
                window.push_notification(Notification::error("Couldn’t start the installer."), cx);
            }
        }
        cx.notify();
    }

    fn render_log(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
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
            .children(self.install.log.iter().cloned())
            .into_any_element()
    }
}

impl Render for ClaudeBanner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let detail = match self.app.read(cx).claude() {
            AgentStatus::Installed { .. } => return div().into_any_element(),
            AgentStatus::Missing => None,
            AgentStatus::Broken { path, error } => Some(format!("{}: {error}", path.display())),
        };
        let install = &self.install;
        let message = if install.last_exit == Some(Some(0)) {
            "Claude Code was installed, but Agent Desk can’t find it yet. Restart Agent Desk, or set AGENT_DESK_CLAUDE_BIN.".to_owned()
        } else {
            detail.unwrap_or_else(|| "Install it to start working with an agent.".to_owned())
        };
        let theme = cx.theme();

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

        v_flex()
            .border_b_1()
            .border_color(theme.border)
            .child(row)
            .when(install.is_log_open, |this| this.child(self.render_log(cx)))
            .into_any_element()
    }
}

/// The installer as one shell-style line, quoting arguments with spaces so
/// the confirmation shows the exact invocation.
fn installer_command_line() -> String {
    let (program, args) = install_command(current_os());
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
    format!("{program} {}", quoted.join(" "))
}
