//! Agent Desk: a desktop app for running coding agents per project thread.

use std::path::PathBuf;
use std::process::ExitCode;

use agent_desk::app_state::AppState;
use agent_desk::appearance::appearance_from_env;
use agent_desk::workspace_view::WorkspaceView;
use agent_desk::{actions, approver_mcp, theme};
use agent_desk_core::approval::{ApprovalServer, ApproverEndpoint, new_token, socket_name};
use agent_desk_core::store::default_root;
use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

/// Points Agent Desk at a specific `claude` binary instead of searching `PATH`.
const CLAUDE_BIN_ENV: &str = "AGENT_DESK_CLAUDE_BIN";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // Claude runs this binary as its approval tool; that mode has no window.
    if args.get(1).map(String::as_str) == Some(approver_mcp::SUBCOMMAND) {
        return approver_mcp::run(&args[2..]);
    }

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
                let (approver, approval_requests) = start_approvals().unzip();
                let app = cx.new(|cx| {
                    let app = AppState::load(root, claude_override);
                    match (approver, approval_requests) {
                        (Some(approver), Some(requests)) => {
                            let mut app = app.with_approver(approver);
                            app.listen_for_approvals(requests, cx);
                            app
                        }
                        _ => app,
                    }
                });
                cx.new(|cx| WorkspaceView::new(app, appearance_override, window, cx))
            })
            .expect("failed to open the Agent Desk window");
        });
    ExitCode::SUCCESS
}

/// Starts the server supervised threads ask before Claude uses a tool. If
/// it can't start, supervised threads refuse to run rather than run
/// unsupervised.
fn start_approvals() -> Option<(ApproverEndpoint, agent_desk::app_state::ApprovalRequests)> {
    let exe = std::env::current_exe()
        .inspect_err(|error| log::warn!("approvals unavailable: {error}"))
        .ok()?;
    let (socket, token) = (socket_name(), new_token());
    let (_server, requests) = ApprovalServer::bind(&socket, &token)
        .inspect_err(|error| log::warn!("approvals unavailable: {error}"))
        .ok()?;
    Some((ApproverEndpoint { exe, socket, token }, requests))
}
