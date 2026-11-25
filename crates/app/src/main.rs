//! Agent Desk: a desktop app for running coding agents per project thread.

use std::path::PathBuf;

use agent_desk::app_state::AppState;
use agent_desk::appearance::appearance_from_env;
use agent_desk::workspace_view::WorkspaceView;
use agent_desk::{actions, theme};
use agent_desk_core::store::default_root;
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

/// Points Agent Desk at a specific `claude` binary instead of searching `PATH`.
const CLAUDE_BIN_ENV: &str = "AGENT_DESK_CLAUDE_BIN";

fn main() {
    let claude_override = std::env::var_os(CLAUDE_BIN_ENV).map(PathBuf::from);
    let root = default_root();
    let appearance_override = appearance_from_env();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::install(cx);
            actions::bind_keys(cx);

            // Window geometry is a platform boundary, so pixels are correct here.
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1100.), px(720.)),
                    cx,
                ))),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Agent Desk".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                let app = cx.new(|_| AppState::load(root, claude_override));
                cx.new(|cx| WorkspaceView::new(app, appearance_override, window, cx))
            })
            .expect("failed to open the Agent Desk window");
        });
}
