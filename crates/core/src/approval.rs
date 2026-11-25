//! Tool approvals between Claude and the person at the app.
//!
//! In supervised mode Claude asks the `agent-desk mcp-approver` subprocess
//! before using a tool. That subprocess forwards the request over a local
//! socket to the running app, which shows it and sends the decision back:
//! one JSON line each way, one connection per request.
//!
//! Local socket names are not private (on Linux, any local user can connect
//! to an abstract socket), so every request carries a per-run token, and the
//! name has a random part so no one can claim it before the app does. Every
//! failure — a wrong token, a malformed or slow request, a request the app
//! drops — ends as a deny.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::{is_safe_file_stem, write_atomic};
use crate::thread::ThreadId;

/// The permission-prompt tool as Claude names it: server `desk`, tool `approve`.
pub const APPROVE_TOOL: &str = "mcp__desk__approve";
/// Carries the token to the approver; a process's environment, unlike its
/// arguments, is private to its user.
pub const TOKEN_ENV: &str = "AGENT_DESK_APPROVAL_TOKEN";

/// How long a client has to send its request line once connected.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Tool inputs can carry a whole file being written, so allow a large line.
const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;

/// Claude wants to use a tool in a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub thread_id: ThreadId,
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    Allow,
    Deny { message: String },
}

impl ApprovalDecision {
    pub fn deny(message: impl Into<String>) -> Self {
        Self::Deny {
            message: message.into(),
        }
    }
}

/// The text the permission-prompt tool returns to Claude. Allow passes the
/// input through unchanged.
pub fn tool_result_json(decision: &ApprovalDecision, input: &Value) -> String {
    match decision {
        ApprovalDecision::Allow => json!({"behavior": "allow", "updatedInput": input}),
        ApprovalDecision::Deny { message } => json!({"behavior": "deny", "message": message}),
    }
    .to_string()
}

/// A fresh socket name for this process: `agent-desk-<pid>-<random>`.
pub fn socket_name() -> String {
    format!(
        "agent-desk-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    )
}

/// A fresh secret that approval requests must carry.
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// How Claude reaches this app's approval server.
#[derive(Debug, Clone)]
pub struct ApproverEndpoint {
    /// The `agent-desk` executable, run as `agent-desk mcp-approver`.
    pub exe: PathBuf,
    pub socket: String,
    pub token: String,
}

impl ApproverEndpoint {
    /// Writes `<root>/mcp/<thread>.json` for Claude's `--mcp-config`. The
    /// file holds the token, so it is private to the user.
    pub fn write_mcp_config(&self, root: &Path, thread: &ThreadId) -> io::Result<PathBuf> {
        if !is_safe_file_stem(&thread.0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsafe thread id",
            ));
        }
        let config = json!({"mcpServers": {"desk": {
            "command": self.exe,
            "args": ["mcp-approver", "--socket", self.socket, "--thread", thread.0],
            "env": {TOKEN_ENV: self.token},
        }}});
        let path = root.join("mcp").join(format!("{}.json", thread.0));
        write_atomic(&path, config.to_string().as_bytes())?;
        Ok(path)
    }
}

/// Answers one request. Dropping it unanswered denies the request.
#[derive(Debug)]
pub struct ApprovalReply(async_channel::Sender<ApprovalDecision>);

impl ApprovalReply {
    /// A reply and where its decision arrives.
    pub fn channel() -> (Self, async_channel::Receiver<ApprovalDecision>) {
        let (sender, receiver) = async_channel::bounded(1);
        (Self(sender), receiver)
    }

    pub fn send(self, decision: ApprovalDecision) {
        // The approver may have gone away; then no one is waiting.
        let _ = self.0.try_send(decision);
    }
}

/// The app's end of the bridge. Requests arrive on the receiver from
/// [`ApprovalServer::bind`]; the server accepts until the process exits.
#[derive(Debug)]
pub struct ApprovalServer {
    name: String,
}

#[derive(Serialize, Deserialize)]
struct WireRequest {
    token: String,
    request: ApprovalRequest,
}

impl ApprovalServer {
    pub fn bind(
        name: &str,
        token: &str,
    ) -> io::Result<(
        Self,
        async_channel::Receiver<(ApprovalRequest, ApprovalReply)>,
    )> {
        let listener = ListenerOptions::new()
            .name(name.to_ns_name::<GenericNamespaced>()?)
            .create_sync()?;
        let (requests, receiver) = async_channel::unbounded();
        let token = token.to_owned();
        thread::Builder::new()
            .name("approval-server".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    if requests.is_closed() {
                        return;
                    }
                    match connection {
                        Ok(connection) => {
                            let (requests, token) = (requests.clone(), token.clone());
                            let spawned = thread::Builder::new()
                                .name("approval-request".into())
                                .spawn(move || serve(connection, &token, &requests));
                            if let Err(error) = spawned {
                                log::warn!("approval request thread: {error}");
                            }
                        }
                        Err(error) => log::warn!("approval connection: {error}"),
                    }
                }
            })?;
        Ok((
            Self {
                name: name.to_owned(),
            },
            receiver,
        ))
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

type RequestSender = async_channel::Sender<(ApprovalRequest, ApprovalReply)>;

fn serve(connection: Stream, token: &str, requests: &RequestSender) {
    let decision = match read_request(&connection, token) {
        Ok(request) => {
            let (reply, decided) = ApprovalReply::channel();
            if requests.send_blocking((request, reply)).is_err() {
                ApprovalDecision::deny("Agent Desk is closing")
            } else {
                decided
                    .recv_blocking()
                    .unwrap_or_else(|_| ApprovalDecision::deny("Agent Desk dropped the request"))
            }
        }
        Err(error) => {
            log::warn!("rejected approval request: {error}");
            ApprovalDecision::deny("Request rejected by Agent Desk")
        }
    };
    if let Err(error) = write_line(&connection, &decision) {
        log::warn!("answering approval request: {error}");
    }
}

fn read_request(connection: &Stream, token: &str) -> io::Result<ApprovalRequest> {
    connection.set_recv_timeout(Some(REQUEST_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(connection.take(MAX_REQUEST_BYTES)).read_line(&mut line)?;
    let wire: WireRequest = serde_json::from_str(&line).map_err(invalid)?;
    if !same_secret(wire.token.as_bytes(), token.as_bytes()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wrong token",
        ));
    }
    Ok(wire.request)
}

/// Compares without stopping at the first difference, so timing does not
/// reveal how much of a guess was right.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Asks the app at `name` to decide, waiting as long as the person takes.
pub async fn request_approval(
    name: &str,
    token: &str,
    request: &ApprovalRequest,
) -> io::Result<ApprovalDecision> {
    let (name, wire) = (
        name.to_owned(),
        WireRequest {
            token: token.to_owned(),
            request: request.clone(),
        },
    );
    smol::unblock(move || {
        let connection = Stream::connect(name.as_str().to_ns_name::<GenericNamespaced>()?)?;
        write_line(&connection, &wire)?;
        let mut line = String::new();
        BufReader::new(&connection).read_line(&mut line)?;
        serde_json::from_str(&line).map_err(invalid)
    })
    .await
}

fn write_line(mut connection: &Stream, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(invalid)?;
    line.push(b'\n');
    connection.write_all(&line)?;
    connection.flush()
}

fn invalid(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
