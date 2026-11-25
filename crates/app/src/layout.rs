//! Shared geometry for the thread pane.
//!
//! The conversation, its header and the composer all sit in one centered,
//! readable column. Building each from [`content_column`] keeps their leading
//! and trailing edges on the same spine instead of three similar paddings.

use gpui_kit::{Div, Styled as _, div, rems};

/// Comfortable line length for prose at the default type size.
const CONTENT_MAX_WIDTH_REM: f32 = 48.;

pub fn content_column() -> Div {
    div()
        .w_full()
        .max_w(rems(CONTENT_MAX_WIDTH_REM))
        .mx_auto()
        .px_6()
}
