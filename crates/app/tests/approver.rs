//! `agent-desk mcp-approver`: the MCP server Claude asks before using a tool.
//! These tests speak MCP's JSON-RPC to the real binary over stdio.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use agent_desk_core::approval::{
    ApprovalDecision, ApprovalServer, TOKEN_ENV, new_token, socket_name,
};
use serde_json::{Value, json};

const THREAD: &str = "6d1f3a52-8c1e-4f43-9a55-2f5b7b0f2c11";

struct Approver {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Approver {
    fn spawn(socket: &str, token: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-desk"))
            .args(["mcp-approver", "--socket", socket, "--thread", THREAD])
            .env(TOKEN_ENV, token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = sender.send(line);
            }
        });
        let mut approver = Self {
            child,
            stdin,
            lines,
        };
        approver.handshake();
        approver
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The response to request `id`, skipping notifications.
    fn response(&self, id: u64) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .expect("a response within 10s");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message;
            }
        }
    }

    fn handshake(&mut self) {
        self.send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }}),
        );
        let init = self.response(1);
        assert!(
            init["result"]["capabilities"]["tools"].is_object(),
            "{init}"
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    /// Calls `approve` and returns the decision Claude would read.
    fn approve(&mut self) -> Value {
        self.send(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "approve",
                "arguments": {"tool_name": "Bash", "input": {"command": "ls"}, "tool_use_id": "t1"},
            }}),
        );
        let response = self.response(2);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("text content in {response}"));
        serde_json::from_str(text).unwrap()
    }
}

impl Drop for Approver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn approver_offers_one_approve_tool() {
    let mut approver = Approver::spawn(&socket_name(), &new_token());
    approver.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}));
    let tools = approver.response(3)["result"]["tools"].clone();
    assert_eq!(tools.as_array().map(Vec::len), Some(1), "{tools}");
    assert_eq!(tools[0]["name"], "approve");
    let properties = &tools[0]["inputSchema"]["properties"];
    for field in ["tool_name", "input", "tool_use_id"] {
        assert!(properties.get(field).is_some(), "{field} in {properties}");
    }
}

#[test]
fn approver_relays_the_request_and_the_deny() {
    let (socket, token) = (socket_name(), new_token());
    let (_server, requests) = ApprovalServer::bind(&socket, &token).unwrap();
    let app = thread::spawn(move || {
        let (request, reply) = requests.recv_blocking().unwrap();
        reply.send(ApprovalDecision::deny("no"));
        request
    });

    let decision = Approver::spawn(&socket, &token).approve();
    assert_eq!(decision, json!({"behavior": "deny", "message": "no"}));
    let request = app.join().unwrap();
    assert_eq!(request.thread_id.0, THREAD);
    assert_eq!(request.tool_name, "Bash");
    assert_eq!(request.input, json!({"command": "ls"}));
}

#[test]
fn approver_relays_allow_with_the_input_unchanged() {
    let (socket, token) = (socket_name(), new_token());
    let (_server, requests) = ApprovalServer::bind(&socket, &token).unwrap();
    thread::spawn(move || {
        let (_, reply) = requests.recv_blocking().unwrap();
        reply.send(ApprovalDecision::Allow);
    });
    assert_eq!(
        Approver::spawn(&socket, &token).approve(),
        json!({"behavior": "allow", "updatedInput": {"command": "ls"}})
    );
}

#[test]
fn approver_without_app_denies() {
    assert_eq!(
        Approver::spawn(&socket_name(), &new_token()).approve(),
        json!({"behavior": "deny", "message": "Agent Desk is not running"})
    );
}
