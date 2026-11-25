//! Keyboard commands for the workspace. Each binding dispatches the same
//! method a button calls, so the two paths cannot disagree.

use gpui_kit::{App, KeyBinding, actions};

/// The key context the workspace root registers its actions under.
pub const WORKSPACE_CONTEXT: &str = "Workspace";

// `KeyBinding` in this GPUI version rejects the `secondary-` alias, so each
// shortcut is spelled with the platform's primary modifier.
#[cfg(target_os = "macos")]
const ADD_PROJECT_KEY: &str = "cmd-o";
#[cfg(not(target_os = "macos"))]
const ADD_PROJECT_KEY: &str = "ctrl-o";
#[cfg(target_os = "macos")]
const NEW_THREAD_KEY: &str = "cmd-n";
#[cfg(not(target_os = "macos"))]
const NEW_THREAD_KEY: &str = "ctrl-n";
// The long-standing "cancel" shortcut on macOS, mirrored with Ctrl elsewhere.
#[cfg(target_os = "macos")]
const STOP_THREAD_KEY: &str = "cmd-.";
#[cfg(not(target_os = "macos"))]
const STOP_THREAD_KEY: &str = "ctrl-.";

actions!(
    agent_desk,
    [
        AddProject,
        NewThread,
        StopThread,
        UseSystemAppearance,
        UseLightAppearance,
        UseDarkAppearance,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(ADD_PROJECT_KEY, AddProject, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(NEW_THREAD_KEY, NewThread, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(STOP_THREAD_KEY, StopThread, Some(WORKSPACE_CONTEXT)),
    ]);
}
