//! The message box under a thread: the text, the thread's model, and Send.
//! Enter sends; Shift+Enter adds a line.

use agent_desk_core::claude::MODEL_ALIASES;
use agent_desk_core::thread::ThreadStatus;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window,
};

use crate::app_state::AppState;
use crate::layout::content_column;

const MIN_ROWS: usize = 1;
const MAX_ROWS: usize = 8;

pub struct Composer {
    app: Entity<AppState>,
    input: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(MIN_ROWS, MAX_ROWS)
                .placeholder("Message Claude…")
                .submit_on_enter(true)
        });
        let subscriptions = vec![
            // Re-render when the thread starts or finishes a turn.
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                // Re-render so Send enables and disables with the text.
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
        ];
        Self {
            app,
            input,
            _subscriptions: subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn draft(&self, cx: &Context<Self>) -> String {
        self.input.read(cx).value().trim().to_owned()
    }

    /// Whether the selected thread is mid-turn; the composer waits for it.
    fn is_busy(&self, cx: &Context<Self>) -> bool {
        self.app
            .read(cx)
            .selected_thread()
            .is_some_and(|thread| thread.status == ThreadStatus::Running)
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy(cx) {
            return;
        }
        let text = self.draft(cx);
        let Some(id) = self.app.read(cx).selected().cloned() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.app
            .update(cx, |app, cx| app.send_message(&id, text, cx));
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }
}

impl Composer {
    /// The thread's model, chosen from Claude Code's aliases. Disabled while a
    /// turn runs, because changing it restarts the session.
    fn render_model_picker(&self, is_busy: bool, cx: &Context<Self>) -> impl IntoElement {
        let thread = self.app.read(cx).selected_thread();
        let id = thread.map(|thread| thread.id.clone());
        let current = thread.and_then(|thread| thread.model.clone());
        let label = current
            .as_deref()
            .map_or("Default model".to_owned(), model_label);
        let app = self.app.clone();

        Button::new("model-picker")
            .ghost()
            .xsmall()
            .label(label)
            .tooltip("Model")
            .disabled(is_busy || id.is_none())
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let choices = std::iter::once(None)
                    .chain(MODEL_ALIASES.iter().map(|alias| Some((*alias).to_owned())));
                choices.fold(menu, |menu, choice| {
                    let label = choice.as_deref().map_or("Default".to_owned(), model_label);
                    let is_checked = choice == current;
                    let (app, id) = (app.clone(), id.clone());
                    menu.item(PopupMenuItem::new(label).checked(is_checked).on_click(
                        move |_, _, cx| {
                            if let Some(id) = &id {
                                app.update(cx, |app, cx| app.set_model(id, choice.clone(), cx));
                            }
                        },
                    ))
                })
            })
    }
}

/// "opus" -> "Opus".
fn model_label(alias: &str) -> String {
    let mut chars = alias.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_empty = self.draft(cx).is_empty();
        let is_busy = self.is_busy(cx);
        content_column().pb_4().child(
            v_flex()
                .gap_2()
                .p_2()
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .bg(theme.background)
                .child(
                    Textarea::new(&self.input)
                        .appearance(false)
                        .disabled(is_busy)
                        .aria_label("Message"),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .gap_2()
                        .child(self.render_model_picker(is_busy, cx))
                        .child(
                            Button::new("send")
                                .primary()
                                .small()
                                .icon(IconName::ArrowUp)
                                .accessibility_label("Send")
                                .tooltip("Send (Enter). Shift+Enter adds a line")
                                .disabled(is_empty || is_busy)
                                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                        ),
                ),
        )
    }
}
