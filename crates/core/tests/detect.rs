mod support;

use std::fs;

use agent_desk_core::detect::{AgentStatus, Os, detect_claude, detect_in, install_command};
use support::{fake_claude_path, process_lock};

#[test]
fn override_to_fake_is_installed() {
    let _guard = process_lock();
    let fake = fake_claude_path();
    assert_eq!(
        detect_claude(Some(&fake)),
        AgentStatus::Installed {
            path: fake,
            version: "9.9.9".into()
        }
    );
}

#[test]
fn missing_override_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        detect_claude(Some(&dir.path().join("nope"))),
        AgentStatus::Missing
    );
}

#[test]
fn empty_search_path_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(detect_in(dir.path().as_os_str()), AgentStatus::Missing);
}

#[test]
fn found_on_search_path() {
    let _guard = process_lock();
    let dir = tempfile::tempdir().unwrap();
    let copy = dir
        .path()
        .join(format!("claude{}", std::env::consts::EXE_SUFFIX));
    fs::copy(fake_claude_path(), &copy).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(
        matches!(detect_in(dir.path().as_os_str()), AgentStatus::Installed { ref version, .. } if version == "9.9.9"),
    );
}

#[test]
fn non_executable_is_broken() {
    let _guard = process_lock();
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("claude.txt");
    fs::write(&text, "not a program").unwrap();
    assert!(matches!(detect_claude(Some(&text)), AgentStatus::Broken { path, .. } if path == text));
}

#[test]
fn install_command_unix() {
    assert_eq!(
        install_command(Os::Unix),
        (
            "bash",
            vec!["-c", "curl -fsSL https://claude.ai/install.sh | bash"]
        )
    );
}

#[test]
fn install_command_windows() {
    assert_eq!(
        install_command(Os::Windows),
        (
            "powershell",
            vec![
                "-NoProfile",
                "-Command",
                "irm https://claude.ai/install.ps1 | iex"
            ]
        )
    );
}
