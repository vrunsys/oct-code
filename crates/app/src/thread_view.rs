//! The conversation pane: a virtualized list of a thread's items.
//!
//! Messages render as quiet typographic blocks rather than chat bubbles.
//! Claude's thinking shows in muted text above what it led to, open by
//! default as in Claude Code, and folds away with one click.
//! Tool calls are collapsed disclosure rows; expanded output shows a preview
//! of [`PREVIEW_LINES`] lines with a toggle for the rest, so a 10k-line tool
//! result never lays out in full unless the user asks for it.

use std::collections::HashSet;

use agent_desk_core::thread::{ApprovalState, Compaction, Item, ThreadId, ToolOutcome};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, IntoElement, ListAlignment, ListState,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use serde_json::Value;

use crate::app_state::AppState;
use crate::format;
use crate::layout::content_column;
use crate::markdown::assistant_markdown;

/// How many lines of tool output an expanded tool row shows before "Show all".
pub const PREVIEW_LINES: usize = 200;
const SUMMARY_MAX_CHARS: usize = 80;

/// The first `max` lines of `text`, and the total line count when it was cut.
pub fn preview_lines(text: &str, max: usize) -> (String, Option<usize>) {
    let total = text.lines().count();
    if total <= max {
        return (text.to_owned(), None);
    }
    (
        text.lines().take(max).collect::<Vec<_>>().join("\n"),
        Some(total),
    )
}

/// A one-line description of a tool call: the command or path when the tool
/// has one, otherwise its input as short JSON.
pub fn tool_summary(name: &str, input: &Value) -> String {
    let key = match name {
        "Bash" => Some("command"),
        "Read" | "Edit" | "Write" | "NotebookEdit" => Some("file_path"),
        "Grep" | "Glob" => Some("pattern"),
        "WebFetch" => Some("url"),
        _ => None,
    };
    let text = key
        .and_then(|key| input.get(key))
        .and_then(Value::as_str)
        .map(|value| value.lines().next().unwrap_or_default().to_owned())
        .unwrap_or_else(|| input.to_string());
    truncate_chars(&text, SUMMARY_MAX_CHARS)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

pub struct ThreadView {
    app: Entity<AppState>,
    list: ListState,
    /// The thread and item count the list was last synced to.
    shown: Option<(ThreadId, usize)>,
    /// [`AppState::rewrites`] when the list was last synced.
    rewrites: u64,
    /// Tool-use ids whose output is expanded, and those showing every line.
    expanded: HashSet<String>,
    full_output: HashSet<String>,
    /// Indexes of thinking items the user folded away.
    folded_thinking: HashSet<usize>,
    /// Indexes of compactions whose summary the user opened; they start
    /// closed, since the summary is long.
    open_compactions: HashSet<usize>,
    _app_subscription: Subscription,
}

impl ThreadView {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Overdraw is a render budget in physical pixels, not product geometry.
        let list = ListState::new(0, ListAlignment::Bottom, px(800.));
        let subscription = cx.observe(&app, |this, _, cx| this.sync(cx));
        let mut this = Self {
            app,
            list,
            shown: None,
            rewrites: 0,
            expanded: HashSet::new(),
            full_output: HashSet::new(),
            folded_thinking: HashSet::new(),
            open_compactions: HashSet::new(),
            _app_subscription: subscription,
        };
        this.sync(cx);
        this
    }

    /// Brings the list in line with the selected thread.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let rewrites = app.rewrites();
        let next = app
            .selected_thread()
            .map(|thread| (thread.id.clone(), thread.items.len()));
        let is_rewritten = std::mem::replace(&mut self.rewrites, rewrites) != rewrites;
        match (&self.shown, &next) {
            (Some((shown_id, shown_len)), Some((id, len)))
                if shown_id == id && len >= shown_len && !is_rewritten =>
            {
                // The last message may still be growing as Claude writes it.
                if let Some(last) = shown_len.checked_sub(1) {
                    self.list.remeasure_items(last..*shown_len);
                }
                if len > shown_len {
                    self.list.splice(*shown_len..*shown_len, len - shown_len);
                }
            }
            _ => {
                self.list.reset(next.as_ref().map_or(0, |(_, len)| *len));
                self.expanded.clear();
                self.full_output.clear();
                self.folded_thinking.clear();
                self.open_compactions.clear();
            }
        }
        self.shown = next;
        cx.notify();
    }

    fn toggle_expanded(&mut self, ix: usize, tool_id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(tool_id) {
            self.expanded.insert(tool_id.to_owned());
        }
        self.list.remeasure_items(ix..ix + 1);
        cx.notify();
    }

    fn toggle_thinking(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.folded_thinking.remove(&ix) {
            self.folded_thinking.insert(ix);
        }
        self.list.remeasure_items(ix..ix + 1);
        cx.notify();
    }

    fn toggle_compaction(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.open_compactions.remove(&ix) {
            self.open_compactions.insert(ix);
        }
        self.list.remeasure_items(ix..ix + 1);
        cx.notify();
    }

    fn toggle_full_output(&mut self, ix: usize, tool_id: &str, cx: &mut Context<Self>) {
        if !self.full_output.remove(tool_id) {
            self.full_output.insert(tool_id.to_owned());
        }
        self.list.remeasure_items(ix..ix + 1);
        cx.notify();
    }

    fn render_item(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self
            .app
            .read(cx)
            .selected_thread()
            .and_then(|thread| thread.items.get(ix))
            .cloned()
        else {
            return div().into_any_element();
        };
        let body = match item {
            Item::User(text) => self.render_message("You", text.into_any_element(), true, cx),
            Item::Assistant(text) => {
                let body = assistant_markdown(self.message_id(ix), text).into_any_element();
                self.render_message("Claude", body, false, cx)
            }
            Item::Thinking(text) => self.render_thinking(ix, text, cx),
            Item::Compacted(compaction) => self.render_compaction(ix, compaction, cx),
            Item::Stats(stats) => div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format::turn_summary(&stats))
                .into_any_element(),
            Item::Approval {
                request_id,
                tool_name,
                input,
                state,
            } => self.render_approval(&request_id, &tool_name, &input, state, cx),
            Item::Tool {
                id,
                name,
                input,
                outcome,
            } => self.render_tool(ix, id, name, &input, outcome, cx),
        };
        // A list row is the root of its own layout, so the column needs a
        // full-width parent for its auto margins to center it.
        div()
            .w_full()
            .child(content_column().py_2().child(body))
            .into_any_element()
    }

    /// A stable id for the message at `ix`. Threads only grow by appending,
    /// so a position within a thread never changes meaning.
    fn message_id(&self, ix: usize) -> SharedString {
        let thread = self.shown.as_ref().map_or("", |(id, _)| id.0.as_str());
        format!("message-{thread}-{ix}").into()
    }

    fn render_message(
        &self,
        author: &'static str,
        body: AnyElement,
        is_user: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child(author),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .when(is_user, |this| {
                        this.px_3().py_2().rounded(theme.radius).bg(theme.muted)
                    })
                    .child(body),
            )
            .into_any_element()
    }

    fn render_thinking(&self, ix: usize, text: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let is_open = !self.folded_thinking.contains(&ix);
        v_flex()
            .gap_1()
            .child(
                // In a row, so the button keeps its natural width.
                h_flex().child(
                    Button::new(SharedString::from(format!("thinking-{ix}")))
                        .ghost()
                        .xsmall()
                        .icon(if is_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .label("Thinking")
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_thinking(ix, cx))),
                ),
            )
            .when(is_open, |this| {
                this.child(
                    div()
                        .pl_3()
                        .border_l_1()
                        .border_color(theme.border)
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(assistant_markdown(self.message_id(ix), text)),
                )
            })
            .into_any_element()
    }

    /// A rule across the thread with what happened in the middle of it. The
    /// summary Claude kept opens under it, and reads as Claude's own text.
    fn render_compaction(
        &self,
        ix: usize,
        compaction: Compaction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let label = format::compaction(&compaction);
        let is_open = self.open_compactions.contains(&ix);
        let rule = || div().flex_1().h_px().bg(theme.border);
        let title = if compaction.summary.is_some() {
            Button::new(SharedString::from(format!("compaction-{ix}")))
                .ghost()
                .xsmall()
                .icon(if is_open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_compaction(ix, cx)))
                .into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label)
                .into_any_element()
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(rule())
                    .child(title)
                    .child(rule()),
            )
            .when_some(compaction.summary.filter(|_| is_open), |this, summary| {
                this.child(
                    div()
                        .pl_3()
                        .border_l_1()
                        .border_color(theme.border)
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(assistant_markdown(self.message_id(ix), summary)),
                )
            })
            .into_any_element()
    }

    /// A pending request is a card with Allow and Deny; once decided it
    /// becomes a quiet row like a tool call's.
    fn render_approval(
        &self,
        request_id: &str,
        tool_name: &str,
        input: &Value,
        state: ApprovalState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let summary = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_xs()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.muted_foreground)
            .child(tool_summary(tool_name, input));

        let decided = match state {
            ApprovalState::Pending => None,
            ApprovalState::Allowed => Some((IconName::Check, theme.success, "Allowed")),
            ApprovalState::Denied => Some((IconName::CircleX, theme.danger, "Denied")),
        };
        if let Some((icon, color, label)) = decided {
            return h_flex()
                .gap_2()
                .min_w_0()
                .px_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(tool_name.to_owned()),
                )
                .child(summary)
                .child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(icon).xsmall().text_color(color))
                        .child(label),
                )
                .into_any_element();
        }

        let decide = |allow: bool| {
            let app = self.app.clone();
            let request_id = request_id.to_owned();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| {
                app.update(cx, |app, cx| app.approve(&request_id, allow, cx));
            }
        };
        v_flex()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .bg(theme.background)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(format!("Allow Claude to use {tool_name}?")),
            )
            .child(h_flex().min_w_0().child(summary))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("allow-{request_id}")))
                            .primary()
                            .small()
                            .label("Allow")
                            .on_click(decide(true)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("deny-{request_id}")))
                            .outline()
                            .small()
                            .label("Deny")
                            .on_click(decide(false)),
                    ),
            )
            .into_any_element()
    }

    fn render_tool(
        &self,
        ix: usize,
        id: String,
        name: String,
        input: &Value,
        outcome: Option<ToolOutcome>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let is_expanded = self.expanded.contains(&id);
        let (status_icon, status_color, status_label) = match &outcome {
            None => (IconName::Loader, theme.muted_foreground, "Running"),
            Some(ToolOutcome::Ok(_)) => (IconName::Check, theme.success, "Done"),
            Some(ToolOutcome::Error(_)) => (IconName::CircleX, theme.danger, "Failed"),
        };
        let summary = tool_summary(&name, input);

        let header = h_flex()
            .gap_2()
            .min_w_0()
            .child(
                Button::new(SharedString::from(format!("tool-{id}")))
                    .ghost()
                    .xsmall()
                    .icon(if is_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label(name)
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| this.toggle_expanded(ix, &id, cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(summary),
            )
            .child(
                // The label keeps status readable without relying on the icon color.
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(status_icon).xsmall().text_color(status_color))
                    .child(status_label),
            );

        let output = match (&outcome, is_expanded) {
            (Some(ToolOutcome::Ok(text) | ToolOutcome::Error(text)), true) => Some(text.clone()),
            _ => None,
        };

        v_flex()
            .gap_1()
            .child(header)
            .when_some(output, |this, text| {
                this.child(self.render_output(ix, &id, &text, cx))
            })
            .into_any_element()
    }

    fn render_output(&self, ix: usize, id: &str, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let show_all = self.full_output.contains(id);
        let (shown, total) = if show_all {
            (
                text.to_owned(),
                Some(text.lines().count()).filter(|n| *n > PREVIEW_LINES),
            )
        } else {
            preview_lines(text, PREVIEW_LINES)
        };
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(if shown.is_empty() {
                        "(no output)".to_owned()
                    } else {
                        shown
                    }),
            )
            .when_some(total, |this, total| {
                let id = id.to_owned();
                // In a row, so the button keeps its natural width.
                this.child(
                    h_flex().child(
                        Button::new(SharedString::from(format!("tool-output-{id}")))
                            .ghost()
                            .xsmall()
                            .label(if show_all {
                                format!("Show first {PREVIEW_LINES} lines")
                            } else {
                                format!("Show all ({total} lines)")
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_full_output(ix, &id, cx)
                            })),
                    ),
                )
            })
            .into_any_element()
    }
}

impl Render for ThreadView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_empty = self.shown.as_ref().is_none_or(|(_, len)| *len == 0);
        if is_empty {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Send a message to start.")
                .into_any_element();
        }
        list(self.list.clone(), cx.processor(Self::render_item))
            .size_full()
            .py_4()
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{preview_lines, tool_summary};

    #[test]
    fn preview_short_text_untouched() {
        assert_eq!(preview_lines("a\nb", 200), ("a\nb".to_owned(), None));
    }

    #[test]
    fn preview_truncates_at_200_lines() {
        let text = (1..=1532)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (preview, total) = preview_lines(&text, 200);
        assert_eq!(total, Some(1532));
        assert_eq!(preview.lines().count(), 200);
        assert_eq!(preview.lines().last(), Some("line 200"));
    }

    #[test]
    fn tool_summary_shows_the_command_or_path() {
        assert_eq!(
            tool_summary("Bash", &json!({"command": "cargo test\n--all"})),
            "cargo test"
        );
        assert_eq!(
            tool_summary("Read", &json!({"file_path": "src/main.rs"})),
            "src/main.rs"
        );
    }

    #[test]
    fn tool_summary_falls_back_to_short_json() {
        let summary = tool_summary("Custom", &json!({"query": "x".repeat(200)}));
        assert!(summary.starts_with("{\"query\""), "{summary}");
        assert!(summary.chars().count() <= 81, "{summary}");
    }
}
