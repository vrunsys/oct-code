use std::path::PathBuf;

use agent_desk_core::claude::{ClaudeConfig, PermissionSetup, build_args, is_valid_session_id};

fn config(resume: Option<&str>, permission: PermissionSetup) -> ClaudeConfig {
    ClaudeConfig {
        binary: PathBuf::from("claude"),
        cwd: PathBuf::from("/tmp/project"),
        resume: resume.map(str::to_owned),
        permission,
    }
}

#[test]
fn base_args_full_access() {
    assert_eq!(
        build_args(&config(None, PermissionSetup::FullAccess)),
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "bypassPermissions"
        ]
    );
}

#[test]
fn resume_appends_flag() {
    let args = build_args(&config(Some("abc"), PermissionSetup::FullAccess));
    assert_eq!(&args[args.len() - 2..], ["--resume", "abc"]);
}

#[test]
fn supervised_adds_mcp_flags() {
    let mcp_config = PathBuf::from("/tmp/mcp/t1.json");
    let args = build_args(&config(
        None,
        PermissionSetup::Supervised {
            mcp_config: mcp_config.clone(),
            tool: "mcp__desk__approve".into(),
        },
    ));
    let flag_value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .map(|i| args[i + 1].clone())
    };
    assert_eq!(
        flag_value("--mcp-config"),
        Some(mcp_config.to_string_lossy().into_owned())
    );
    assert_eq!(
        flag_value("--permission-prompt-tool").as_deref(),
        Some("mcp__desk__approve")
    );
    assert!(!args.iter().any(|a| a == "--permission-mode"));
}

#[test]
fn real_session_ids_are_valid() {
    assert!(is_valid_session_id("0222ee39-2e20-4887-a779-9246dfce7c4f"));
    assert!(is_valid_session_id("fake-session-1"));
}

#[test]
fn ids_that_could_inject_arguments_are_invalid() {
    // A saved thread file is outside our control once written; its session id
    // becomes a command-line argument, so it must never look like a flag.
    assert!(!is_valid_session_id("--dangerously-skip-permissions"));
    assert!(!is_valid_session_id("-x"));
    assert!(!is_valid_session_id(""));
    assert!(!is_valid_session_id("a b"));
    assert!(!is_valid_session_id("../etc"));
    assert!(!is_valid_session_id(&"a".repeat(129)));
}
