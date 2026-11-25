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
    pub permission: PermissionSetup,
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

    if let Some(session_id) = &cfg.resume {
        args.extend(["--resume".to_owned(), session_id.clone()]);
    }
    args
}
