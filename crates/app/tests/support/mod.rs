//! Shared by the app's integration tests.

use std::path::PathBuf;

use agent_desk_core::approval::ApproverEndpoint;

/// An approver for supervised threads in tests. fake-claude never starts
/// MCP servers, so only the flags it is given matter.
pub fn test_approver() -> ApproverEndpoint {
    ApproverEndpoint {
        exe: PathBuf::from("/nonexistent/agent-desk"),
        socket: "agent-desk-test".into(),
        token: "test-token".into(),
    }
}
