//! `agent-desk mcp-approver --socket <name> --thread <id>`: the MCP server
//! Claude runs as its permission-prompt tool in supervised threads.
//!
//! It offers one tool, `approve`. Each call is forwarded to the running app,
//! which asks the person and answers. If the app can't be reached, the
//! answer is a deny: Claude never gets permission by default.
//!
//! stdout carries MCP's JSON-RPC, so nothing else may be printed there.

use std::process::ExitCode;

use agent_desk_core::approval::{
    ApprovalDecision, ApprovalRequest, TOKEN_ENV, request_approval, tool_result_json,
};
use agent_desk_core::thread::ThreadId;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt as _, schemars, tool, tool_handler, tool_router};
use serde_json::{Map, Value};

/// The first argument that selects this mode instead of opening a window.
pub const SUBCOMMAND: &str = "mcp-approver";

/// What Claude sends when it wants to use a tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ApproveParams {
    /// The tool Claude wants to use, such as `Bash`.
    tool_name: String,
    /// The tool's input, exactly as it would run.
    input: Map<String, Value>,
    /// Claude's id for this tool use.
    #[expect(dead_code, reason = "part of Claude's call; requests get their own id")]
    tool_use_id: String,
}

#[derive(Debug, Clone)]
struct Approver {
    socket: String,
    token: String,
    thread: ThreadId,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Approver {
    fn new(socket: String, token: String, thread: ThreadId) -> Self {
        Self {
            socket,
            token,
            thread,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Asks the person using Agent Desk whether Claude may use a tool.")]
    async fn approve(&self, Parameters(params): Parameters<ApproveParams>) -> String {
        let input = Value::Object(params.input);
        let request = ApprovalRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            thread_id: self.thread.clone(),
            tool_name: params.tool_name,
            input: input.clone(),
        };
        let decision = request_approval(&self.socket, &self.token, &request)
            .await
            .unwrap_or_else(|error| {
                eprintln!("agent-desk mcp-approver: {error}");
                ApprovalDecision::deny("Agent Desk is not running")
            });
        tool_result_json(&decision, &input)
    }
}

#[tool_handler]
impl ServerHandler for Approver {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Runs the server on stdio until Claude closes it. `args` follow the
/// subcommand.
pub fn run(args: &[String]) -> ExitCode {
    let (socket, thread) = match args {
        [socket_flag, socket, thread_flag, thread]
            if socket_flag == "--socket" && thread_flag == "--thread" =>
        {
            (socket.clone(), ThreadId(thread.clone()))
        }
        _ => {
            eprintln!("usage: agent-desk {SUBCOMMAND} --socket <name> --thread <id>");
            return ExitCode::from(2);
        }
    };
    // Without the token every request would be refused, so say so plainly.
    let Ok(token) = std::env::var(TOKEN_ENV) else {
        eprintln!("agent-desk {SUBCOMMAND}: {TOKEN_ENV} is not set");
        return ExitCode::from(2);
    };

    let served = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
        .and_then(|runtime| {
            runtime.block_on(async {
                let service = Approver::new(socket, token, thread)
                    .serve(rmcp::transport::stdio())
                    .await
                    .map_err(|error| error.to_string())?;
                service
                    .waiting()
                    .await
                    .map(drop)
                    .map_err(|error| error.to_string())
            })
        });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("agent-desk {SUBCOMMAND}: {error}");
            ExitCode::FAILURE
        }
    }
}
