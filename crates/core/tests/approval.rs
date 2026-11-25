//! The approval bridge between `agent-desk mcp-approver` and the app, and
//! approval items on a thread.

use std::path::PathBuf;
use std::thread;

use agent_desk_core::approval::{
    APPROVE_TOOL, ApprovalDecision, ApprovalReply, ApprovalRequest, ApprovalServer,
    ApproverEndpoint, TOKEN_ENV, new_token, request_approval, socket_name, tool_result_json,
};
use agent_desk_core::thread::{ApprovalState, Item, PermissionMode, Thread, ThreadId};
use agent_desk_test_support::block_on_timeout;
use serde_json::{Value, json};

fn request(thread_id: ThreadId) -> ApprovalRequest {
    ApprovalRequest {
        request_id: "r1".into(),
        thread_id,
        tool_name: "Bash".into(),
        input: json!({"command": "ls"}),
    }
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn allow_json_echoes_input() {
    assert_eq!(
        parse(&tool_result_json(
            &ApprovalDecision::Allow,
            &json!({"command": "ls"})
        )),
        json!({"behavior": "allow", "updatedInput": {"command": "ls"}})
    );
}

#[test]
fn deny_json_has_message() {
    let deny = ApprovalDecision::Deny {
        message: "no".into(),
    };
    assert_eq!(
        parse(&tool_result_json(&deny, &json!({}))),
        json!({"behavior": "deny", "message": "no"})
    );
}

#[test]
fn socket_names_are_unpredictable() {
    let (a, b) = (socket_name(), socket_name());
    assert!(
        a.starts_with(&format!("agent-desk-{}-", std::process::id())),
        "{a}"
    );
    assert_ne!(a, b);
}

#[test]
fn bridge_roundtrip() {
    let (name, token) = (socket_name(), new_token());
    let (_server, requests) = ApprovalServer::bind(&name, &token).unwrap();
    let id = ThreadId::new();
    let app = thread::spawn(move || {
        let (received, reply) = requests.recv_blocking().unwrap();
        reply.send(ApprovalDecision::Allow);
        received
    });

    let decision = block_on_timeout(request_approval(&name, &token, &request(id.clone())));
    assert_eq!(decision.unwrap(), ApprovalDecision::Allow);
    assert_eq!(app.join().unwrap(), request(id));
}

#[test]
fn a_request_without_the_token_is_denied_and_never_reaches_the_app() {
    let name = socket_name();
    let (_server, requests) = ApprovalServer::bind(&name, &new_token()).unwrap();
    let decision = block_on_timeout(request_approval(
        &name,
        &new_token(),
        &request(ThreadId::new()),
    ))
    .unwrap();
    assert!(
        matches!(decision, ApprovalDecision::Deny { .. }),
        "{decision:?}"
    );
    assert!(requests.is_empty());
}

#[test]
fn a_request_the_app_drops_is_denied() {
    let (name, token) = (socket_name(), new_token());
    let (_server, requests) = ApprovalServer::bind(&name, &token).unwrap();
    let app = thread::spawn(move || drop(requests.recv_blocking().unwrap()));
    let decision =
        block_on_timeout(request_approval(&name, &token, &request(ThreadId::new()))).unwrap();
    app.join().unwrap();
    assert!(
        matches!(decision, ApprovalDecision::Deny { .. }),
        "{decision:?}"
    );
}

#[test]
fn client_without_server_errors() {
    let result = block_on_timeout(request_approval(
        &socket_name(),
        &new_token(),
        &request(ThreadId::new()),
    ));
    assert!(result.is_err());
}

#[test]
fn push_and_resolve_approval() {
    let mut thread = Thread::new(PathBuf::from("/tmp/project"));
    thread.push_approval(&request(thread.id.clone()));
    assert_eq!(
        thread.items.last(),
        Some(&Item::Approval {
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            input: json!({"command": "ls"}),
            state: ApprovalState::Pending,
        })
    );
    assert!(thread.resolve_approval("r1", true));
    assert!(matches!(
        thread.items.last(),
        Some(Item::Approval {
            state: ApprovalState::Allowed,
            ..
        })
    ));
    assert!(!thread.resolve_approval("r1", false), "already decided");
    assert!(!thread.resolve_approval("missing", true));
}

#[test]
fn threads_are_supervised_unless_saved_otherwise() {
    assert_eq!(
        Thread::new(PathBuf::from("/tmp/project")).permission_mode,
        PermissionMode::Supervised
    );
    // A thread saved before modes existed has no field and loads supervised.
    let mut saved = serde_json::to_value(Thread::new(PathBuf::from("/tmp/p"))).unwrap();
    saved.as_object_mut().unwrap().remove("permission_mode");
    let loaded: Thread = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.permission_mode, PermissionMode::Supervised);
}

fn endpoint() -> ApproverEndpoint {
    ApproverEndpoint {
        exe: PathBuf::from("/opt/agent desk/agent-desk"),
        socket: "agent-desk-1-abc".into(),
        token: "secret".into(),
    }
}

#[test]
fn mcp_config_runs_the_approver_with_the_token_in_its_environment() {
    let root = tempfile::tempdir().unwrap();
    let id = ThreadId::new();
    let path = endpoint().write_mcp_config(root.path(), &id).unwrap();
    assert_eq!(path, root.path().join("mcp").join(format!("{}.json", id.0)));
    assert_eq!(APPROVE_TOOL, "mcp__desk__approve");

    let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        config,
        json!({"mcpServers": {"desk": {
            "command": "/opt/agent desk/agent-desk",
            "args": ["mcp-approver", "--socket", "agent-desk-1-abc", "--thread", id.0],
            "env": {TOKEN_ENV: "secret"},
        }}})
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the token must stay private");
    }
}

#[test]
fn mcp_config_refuses_a_thread_id_that_could_escape_its_folder() {
    let root = tempfile::tempdir().unwrap();
    let error = endpoint()
        .write_mcp_config(root.path(), &ThreadId("../escape".into()))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn a_reply_delivers_one_decision() {
    let (reply, decided) = ApprovalReply::channel();
    reply.send(ApprovalDecision::Allow);
    assert_eq!(decided.try_recv(), Ok(ApprovalDecision::Allow));
}
