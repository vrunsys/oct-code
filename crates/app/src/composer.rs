//! The message box under a thread. Enter sends; Shift+Enter adds a line.

use agent_desk_core::thread::ThreadStatus;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div,
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
                        .child(
                            div()
                                .px_1()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Enter to send, Shift+Enter for a new line"),
                        )
                        .child(
                            Button::new("send")
                                .primary()
                                .small()
                                .label("Send")
                                .disabled(is_empty || is_busy)
                                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                        ),
                ),
        )
    }
}
