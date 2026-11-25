//! The message box under a thread: the text, then the model picker and Send
//! on the trailing edge.
//! Enter sends; Shift+Enter adds a line.

use agent_desk_core::claude::catalog::ModelCatalog;
use agent_desk_core::thread::{ThreadId, ThreadStatus};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex,
    v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
    rems,
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

/// Width of the model column; the reasoning column is wider for its notes.
const MODEL_COLUMN_REM: f32 = 11.;
const REASONING_COLUMN_REM: f32 = 14.;

impl Composer {
    /// The thread's model and reasoning level, side by side in one popover,
    /// both from Claude Code's catalog. It stays open while you choose, so a
    /// model and its level can be picked in one go. Disabled while a turn
    /// runs, because a change restarts the session.
    fn render_model_picker(&self, is_busy: bool, cx: &Context<Self>) -> AnyElement {
        let app_state = self.app.read(cx);
        let thread = app_state.selected_thread();
        let label = picker_label(
            app_state.catalog(),
            thread.and_then(|thread| thread.model.as_deref()),
            thread.and_then(|thread| thread.effort.as_deref()),
        );
        let trigger = Button::new("model-picker")
            .ghost()
            .xsmall()
            .label(label)
            .tooltip("Model and reasoning");
        let Some(id) = thread.map(|thread| thread.id.clone()).filter(|_| !is_busy) else {
            return trigger.disabled(true).into_any_element();
        };

        let app = self.app.clone();
        Popover::new("model-picker-popover")
            .anchor(Anchor::BottomRight)
            .trigger(trigger)
            .content(move |_, _, cx| render_picker_columns(&app, &id, cx))
            .into_any_element()
    }
}

/// The Model and Reasoning columns, read fresh from the app on every render
/// so a choice shows its check at once.
fn render_picker_columns(app: &Entity<AppState>, id: &ThreadId, cx: &mut App) -> AnyElement {
    let state = app.read(cx);
    let Some(thread) = state.threads().iter().find(|thread| &thread.id == id) else {
        return div().into_any_element();
    };
    let model = thread.model.clone();
    let effort = thread.effort.clone();
    let catalog = state.catalog().clone();
    let border = cx.theme().border;

    let model_rows = std::iter::once(None)
        .chain(catalog.models.iter().map(Some))
        .map(|info| {
            let choice = info.map(|info| info.id.clone());
            let name = info.map_or("Default", |info| info.name.as_str());
            let (app, id) = (app.clone(), id.clone());
            picker_row(
                row_id("model", choice.as_deref()),
                name,
                None,
                choice == model,
                cx,
            )
            .on_click(move |_, _, cx| {
                app.update(cx, |app, cx| app.set_model(&id, choice.clone(), cx))
            })
        });

    let levels = catalog.efforts_for(model.as_deref());
    let reasoning: Vec<AnyElement> = if levels.is_empty() {
        vec![muted_note("Not available for this model", cx)]
    } else {
        std::iter::once(None)
            .chain(levels.into_iter().map(Some))
            .map(|level| {
                let choice = level.as_ref().map(|level| level.id.clone());
                let name = level
                    .as_ref()
                    .map_or("Default".to_owned(), |level| level.name.clone());
                let note = level.and_then(|level| level.note);
                let (app, id) = (app.clone(), id.clone());
                picker_row(
                    row_id("effort", choice.as_deref()),
                    &name,
                    note,
                    choice == effort,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    app.update(cx, |app, cx| app.set_effort(&id, choice.clone(), cx))
                })
                .into_any_element()
            })
            .collect()
    };

    // Stretch both columns so the divider runs the popover's full height.
    h_flex()
        .items_stretch()
        .child(
            v_flex()
                .w(rems(MODEL_COLUMN_REM))
                .pr_1()
                .child(column_title("Model", cx))
                .children(model_rows),
        )
        .child(
            v_flex()
                .w(rems(REASONING_COLUMN_REM))
                .pl_1()
                .border_l_1()
                .border_color(border)
                .child(column_title("Reasoning", cx))
                .children(reasoning),
        )
        .into_any_element()
}

/// One choice: a fixed check slot keeps every label on the same edge, and an
/// optional note (such as "Recommended") sits on the trailing edge.
fn picker_row(
    id: SharedString,
    name: &str,
    note: Option<String>,
    is_checked: bool,
    cx: &App,
) -> Button {
    let theme = cx.theme();
    Button::new(id)
        .ghost()
        .small()
        .w_full()
        .selected(is_checked)
        .accessibility_label(name.to_owned())
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(div().size_4().flex_none().when(is_checked, |this| {
                    this.child(Icon::new(IconName::Check).xsmall())
                }))
                .child(div().flex_1().min_w_0().truncate().child(name.to_owned()))
                .when_some(note, |this, note| {
                    this.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(note),
                    )
                }),
        )
}

fn column_title(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .pb_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

fn muted_note(text: &'static str, cx: &App) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

/// A stable element id for a catalog choice in a column, e.g.
/// `model:claude-opus-5-5`, or `model:default` for no choice.
fn row_id(column: &str, choice: Option<&str>) -> SharedString {
    format!("{column}:{}", choice.unwrap_or("default")).into()
}

/// "Opus 5.5 · High", or "Default model" when nothing is chosen.
fn picker_label(catalog: &ModelCatalog, model: Option<&str>, effort: Option<&str>) -> String {
    let model_name = model
        .and_then(|id| catalog.model(id))
        .map_or("Default model", |info| info.name.as_str());
    let effort_name = effort.and_then(|id| {
        catalog
            .efforts_for(model)
            .into_iter()
            .find(|level| level.id == id)
            .map(|level| level.name)
    });
    match effort_name {
        Some(effort) => format!("{model_name} · {effort}"),
        None => model_name.to_owned(),
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
                        .justify_end()
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
