//! One long-lived `claude -p` process per thread, spoken to over stream-json.

use std::collections::VecDeque;
use std::path::PathBuf;

use async_channel::{Receiver, Sender};
use serde_json::json;
use smol::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use smol::lock::Mutex;
use smol::process::{Child, ChildStdin, Command, Stdio};

use super::{AgentEvent, ClaudeConfig, build_args, parse_line};

/// How many trailing stderr lines an [`SessionEvent::Exited`] carries.
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Agent(AgentEvent),
    /// The process ended. Always the last event; every `Agent` event comes before it.
    Exited {
        code: Option<i32>,
        stderr_tail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    NotFound(PathBuf),
    CwdMissing(PathBuf),
    Io(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotFound(path) => write!(f, "agent binary not found: {}", path.display()),
            SessionError::CwdMissing(path) => {
                write!(f, "project folder not found: {}", path.display())
            }
            SessionError::Io(error) => write!(f, "agent I/O error: {error}"),
        }
    }
}

impl std::error::Error for SessionError {}

pub struct ClaudeSession {
    child: Child,
    stdin: Mutex<Option<ChildStdin>>,
}

impl ClaudeSession {
    /// Spawns Claude in `cfg.cwd`. Events arrive on the returned receiver
    /// until the process exits.
    pub fn start(
        cfg: &ClaudeConfig,
    ) -> Result<(ClaudeSession, Receiver<SessionEvent>), SessionError> {
        if !cfg.cwd.is_dir() {
            return Err(SessionError::CwdMissing(cfg.cwd.clone()));
        }

        let mut command = Command::new(&cfg.binary);
        command
            .args(build_args(cfg))
            .current_dir(&cfg.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console_window(&mut command);

        let mut child = command.spawn().map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => SessionError::NotFound(cfg.binary.clone()),
            _ => SessionError::Io(error.to_string()),
        })?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (events, receiver) = async_channel::unbounded();

        let stdout_task = smol::spawn(forward_stdout(stdout, events.clone()));
        let stderr_task = smol::spawn(collect_stderr_tail(stderr));
        let status = child.status();
        smol::spawn(async move {
            // Drain stdout first so `Exited` is always the last event.
            stdout_task.await;
            let stderr_tail = stderr_task.await;
            let code = status.await.ok().and_then(|status| status.code());
            let _ = events
                .send(SessionEvent::Exited { code, stderr_tail })
                .await;
        })
        .detach();

        Ok((
            ClaudeSession {
                child,
                stdin: Mutex::new(stdin),
            },
            receiver,
        ))
    }

    /// Sends one user message. The text travels as JSON over stdin, never as
    /// a command-line argument.
    pub async fn send(&self, text: &str) -> Result<(), SessionError> {
        let message = json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        });
        let mut line = message.to_string();
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| SessionError::Io("session is stopped".into()))?;
        stdin.write_all(line.as_bytes()).await.map_err(io_error)?;
        stdin.flush().await.map_err(io_error)
    }

    /// Kills the process. The event stream still ends with `Exited`.
    pub fn stop(&mut self) {
        self.stdin.get_mut().take();
        if let Err(error) = self.child.kill() {
            // Already exited is the common case here.
            log::debug!("killing agent process: {error}");
        }
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn forward_stdout(stdout: smol::process::ChildStdout, events: Sender<SessionEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) => {
                log::warn!("reading agent stdout: {error}");
                return;
            }
        }
        let line = String::from_utf8_lossy(&buffer);
        match parse_line(&line) {
            Ok(parsed) => {
                for event in parsed {
                    // A dropped receiver is fine: keep draining so the child never blocks on a full pipe.
                    let _ = events.send(SessionEvent::Agent(event)).await;
                }
            }
            Err(error) => log::warn!("skipping agent stdout line: {error}"),
        }
    }
}

async fn collect_stderr_tail(stderr: smol::process::ChildStderr) -> String {
    let mut reader = BufReader::new(stderr);
    let mut tail = VecDeque::with_capacity(STDERR_TAIL_LINES);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(String::from_utf8_lossy(&buffer).trim_end().to_owned());
            }
        }
    }
    Vec::from(tail).join("\n")
}

fn io_error(error: std::io::Error) -> SessionError {
    SessionError::Io(error.to_string())
}

/// A GUI app spawning a console program on Windows flashes a console window
/// unless told not to.
#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use smol::process::windows::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}
