//! [`ClaudeConfig`] to the Claude Code CLI's argument list.
//!
//! User text never appears here: messages go over stdin as stream-json.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum PermissionSetup {
    /// Claude runs every tool without asking.
    FullAccess,
    /// Claude asks `tool` on the MCP server described by `mcp_config` before each tool call.
    Supervised { mcp_config: PathBuf, tool: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeConfig {
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub resume: Option<String>,
    /// A model alias from [`MODEL_ALIASES`]; anything else is not passed.
    pub model: Option<String>,
    pub permission: PermissionSetup,
}

/// The model aliases Agent Desk offers, as Claude Code's `--model` takes them.
pub const MODEL_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// Whether `model` is one of [`MODEL_ALIASES`].
pub fn is_known_model(model: &str) -> bool {
    MODEL_ALIASES.contains(&model)
}

pub fn build_args(cfg: &ClaudeConfig) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ]
    .map(str::to_owned)
    .into();

    match &cfg.permission {
        PermissionSetup::FullAccess => {
            args.extend([
                "--permission-mode".to_owned(),
                "bypassPermissions".to_owned(),
            ]);
        }
        PermissionSetup::Supervised { mcp_config, tool } => {
            args.extend([
                "--mcp-config".to_owned(),
                mcp_config.to_string_lossy().into_owned(),
                "--permission-prompt-tool".to_owned(),
                tool.clone(),
            ]);
        }
    }

    if let Some(model) = cfg.model.as_deref().filter(|model| is_known_model(model)) {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if let Some(session_id) = &cfg.resume {
        args.extend(["--resume".to_owned(), session_id.clone()]);
    }
    args
}

/// Longest session id accepted for `--resume`; real ids are 36-char UUIDs.
const SESSION_ID_MAX_LEN: usize = 128;

/// Whether `id` is safe to pass as the value of `--resume`.
///
/// Session ids come back from saved thread files, which are outside the app's
/// control once written. An id must start with a letter or digit and contain
/// only letters, digits, `-` and `_`, so it can never be read as a flag.
pub fn is_valid_session_id(id: &str) -> bool {
    id.len() <= SESSION_ID_MAX_LEN
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
