//! Is Claude Code installed, and how to install it.
//!
//! Detection runs `<claude> --version` with a timeout, because the UI thread
//! waits for it. The installer runs a fixed command and only when the user
//! clicks Install; nothing here builds a command from user input.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use smol::io::{AsyncBufReadExt, BufReader};

use crate::platform::background_command;

const BINARY_NAME: &str = "claude";
/// Long enough for a cold start of a Node-based CLI, short enough not to hang the UI.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Installed { path: PathBuf, version: String },
    Missing,
    Broken { path: PathBuf, error: String },
}

/// Checks `override_bin` if given, otherwise looks for `claude` on `PATH`.
pub fn detect_claude(override_bin: Option<&Path>) -> AgentStatus {
    match override_bin {
        Some(path) if !path.exists() => AgentStatus::Missing,
        Some(path) => probe(path, VERSION_TIMEOUT),
        None => which::which(BINARY_NAME)
            .map_or(AgentStatus::Missing, |path| probe(&path, VERSION_TIMEOUT)),
    }
}

/// Looks for `claude` only in `search_path` (a `PATH`-style list).
pub fn detect_in(search_path: &OsStr) -> AgentStatus {
    let Ok(cwd) = std::env::current_dir() else {
        return AgentStatus::Missing;
    };
    which::which_in(BINARY_NAME, Some(search_path), cwd)
        .map_or(AgentStatus::Missing, |path| probe(&path, VERSION_TIMEOUT))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Unix,
    Windows,
}

pub fn current_os() -> Os {
    if cfg!(windows) { Os::Windows } else { Os::Unix }
}

/// The official installer for `os`, as a program and its arguments.
pub fn install_command(os: Os) -> (&'static str, Vec<&'static str>) {
    match os {
        Os::Unix => (
            "bash",
            vec!["-c", "curl -fsSL https://claude.ai/install.sh | bash"],
        ),
        Os::Windows => (
            "powershell",
            vec![
                "-NoProfile",
                "-Command",
                "irm https://claude.ai/install.ps1 | iex",
            ],
        ),
    }
}

/// Runs the official installer, passing each stdout and stderr line to
/// `on_line`, and returns its exit code. Call only after an explicit click.
pub async fn run_install(os: Os, on_line: impl FnMut(String)) -> io::Result<Option<i32>> {
    let (program, args) = install_command(os);
    let mut command = background_command(program);
    command.args(args);
    stream_lines(command, on_line).await
}

/// Runs `<path> --version` and reads the version from the first word of stdout.
fn probe(path: &Path, timeout: Duration) -> AgentStatus {
    let broken = |error: String| AgentStatus::Broken {
        path: path.to_path_buf(),
        error,
    };

    let mut command = background_command(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return broken(format!("could not run: {error}")),
    };

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return broken(format!(
                    "`--version` timed out after {}s",
                    timeout.as_secs_f32()
                ));
            }
            Err(error) => return broken(format!("waiting for `--version`: {error}")),
        }
    };
    if !status.success() {
        return broken(match status.code() {
            Some(code) => format!("`--version` exited with code {code}"),
            None => "`--version` was killed".to_owned(),
        });
    }

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = io::Read::read_to_string(&mut pipe, &mut stdout);
    }
    match stdout.split_whitespace().next() {
        Some(version) => AgentStatus::Installed {
            path: path.to_path_buf(),
            version: version.to_owned(),
        },
        None => broken("`--version` printed nothing".to_owned()),
    }
}

/// Runs `command` to completion, passing its stdout and stderr lines to `on_line`.
async fn stream_lines(
    command: std::process::Command,
    mut on_line: impl FnMut(String),
) -> io::Result<Option<i32>> {
    let mut child = smol::process::Command::from(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let (lines, received) = async_channel::unbounded();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let readers = [
        smol::spawn(forward_lines(BufReader::new(stdout), lines.clone())),
        smol::spawn(forward_lines(BufReader::new(stderr), lines)),
    ];
    // Ends once both readers have dropped their senders, at EOF on both pipes.
    while let Ok(line) = received.recv().await {
        on_line(line);
    }
    for reader in readers {
        reader.await;
    }
    Ok(child.status().await?.code())
}

async fn forward_lines(
    mut reader: impl AsyncBufReadExt + Unpin,
    lines: async_channel::Sender<String>,
) {
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buffer).trim_end().to_owned();
                if lines.send(line).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests write scripts and spawn processes on parallel threads. A
    /// fork on one thread can briefly inherit another thread's open write
    /// handle to a script, and exec then fails with ETXTBSY. Holding this
    /// lock for each test's whole body rules that race out.
    static PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        PROCESS_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn hanging_version_check_times_out() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "exec sleep 30");
        let started = Instant::now();
        let status = probe(&path, Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "probe waited {:?}",
            started.elapsed()
        );
        assert!(
            matches!(&status, AgentStatus::Broken { error, .. } if error.contains("timed out")),
            "{status:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failing_version_check_is_broken() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "exit 2");
        let status = probe(&path, VERSION_TIMEOUT);
        assert!(
            matches!(&status, AgentStatus::Broken { error, .. } if error.contains("code 2")),
            "{status:?}"
        );
    }

    #[test]
    fn stream_lines_reports_stdout_stderr_and_exit_code() {
        let _guard = lock();
        #[cfg(unix)]
        let mut command = background_command("sh");
        #[cfg(unix)]
        command.args(["-c", "echo out; echo err >&2; exit 3"]);
        #[cfg(windows)]
        let mut command = background_command("cmd");
        #[cfg(windows)]
        command.args(["/C", "echo out& echo err 1>&2& exit 3"]);

        let mut lines = Vec::new();
        let code = smol::block_on(stream_lines(command, |line| lines.push(line))).unwrap();
        lines.sort();
        assert_eq!(lines, ["err", "out"]);
        assert_eq!(code, Some(3));
    }
}
