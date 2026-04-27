//! MCP client — JSON-RPC handshake + tool dispatch over a
//! `JsonRpcTransport`.
//!
//! Lifecycle:
//!   1. Construct via `McpClient::new(transport)` — performs the
//!      `initialize` handshake and sends the `notifications/initialized`
//!      notification.
//!   2. `list_tools()` discovers the server's surface.
//!   3. `call_tool(name, arguments)` invokes a tool.
//!   4. Drop kills the subprocess (transport's responsibility).
//!
//! Error model:
//!   - `McpError::Spawn` — couldn't start the subprocess.
//!   - `McpError::Transport` — IO failure on the byte channel.
//!   - `McpError::Protocol` — wire-level violation (malformed JSON,
//!     missing `result`+`error`, mismatched id, etc.).
//!   - `McpError::JsonRpc { code, message }` — server returned a
//!     JSON-RPC error.
//!
//! NO auto-retry on any of these. The agent surfaces them; the user
//! decides whether to start a fresh session.

use serde_json::{json, Value};
use std::fmt::{Display, Formatter};

use super::protocol::{
    InitializeResult, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ToolsCallResult,
    ToolsListResult, WireContent, WireTool, CLIENT_PROTOCOL_VERSION,
};
use super::transport::JsonRpcTransport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    Spawn(String),
    Transport(String),
    Protocol(String),
    JsonRpc { code: i32, message: String },
}

impl Display for McpError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(message) => write!(f, "MCP spawn failed: {message}"),
            Self::Transport(message) => write!(f, "MCP transport error: {message}"),
            Self::Protocol(message) => write!(f, "MCP protocol error: {message}"),
            Self::JsonRpc { code, message } => write!(f, "MCP JSON-RPC error {code}: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

/// One tool advertised by an MCP server, in aegis-agent shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Result of one `tools/call`. `text` is the concatenated text
/// content of the response (the typical MCP shape); `is_error`
/// reflects the server's `isError` field; `structured` carries any
/// optional `structuredContent` payload.
#[derive(Clone, Debug)]
pub struct McpToolResult {
    pub text: String,
    pub is_error: bool,
    pub structured: Option<Value>,
}

pub struct McpClient {
    transport: Box<dyn JsonRpcTransport>,
    next_id: u64,
    /// Server-reported info captured at handshake time, exposed for
    /// diagnostics.
    pub server_name: String,
    pub server_version: String,
    pub server_protocol_version: String,
}

impl McpClient {
    /// Construct a client over an existing transport. Performs the
    /// MCP `initialize` handshake before returning. If anything in
    /// the handshake fails, the client is NOT created — the caller
    /// gets the error and decides what to do.
    pub fn new(mut transport: Box<dyn JsonRpcTransport>) -> Result<Self, McpError> {
        let mut next_id = 1_u64;

        // 1. Send `initialize` request.
        let init_id = next_id;
        next_id += 1;
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: init_id,
            method: "initialize",
            params: json!({
                "protocolVersion": CLIENT_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "aegis-agent",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        };
        let init_request_str = serde_json::to_string(&init_request)
            .map_err(|e| McpError::Protocol(format!("serialise initialize: {e}")))?;
        transport.send(&init_request_str)?;

        // 2. Read `initialize` response.
        let init_response_str = transport.recv()?;
        let init_response: JsonRpcResponse = serde_json::from_str(&init_response_str)
            .map_err(|e| McpError::Protocol(format!("parse initialize response: {e}")))?;
        let init_result = unwrap_result::<InitializeResult>(init_response, init_id, "initialize")?;

        // 3. Send `notifications/initialized` (no response expected).
        let initialized = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized",
            params: json!({}),
        };
        let initialized_str = serde_json::to_string(&initialized)
            .map_err(|e| McpError::Protocol(format!("serialise initialized: {e}")))?;
        transport.send(&initialized_str)?;

        let server_info = init_result.server_info.unwrap_or_default();
        Ok(Self {
            transport,
            next_id,
            server_name: server_info.name,
            server_version: server_info.version,
            server_protocol_version: init_result.protocol_version,
        })
    }

    /// Discover the tools this server advertises.
    pub fn list_tools(&mut self) -> Result<Vec<McpTool>, McpError> {
        let id = self.fresh_id();
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: "tools/list",
            params: json!({}),
        };
        let response = self.round_trip::<ToolsListResult>(&request, "tools/list")?;
        Ok(response.tools.into_iter().map(into_mcp_tool).collect())
    }

    /// Invoke a tool. `arguments` is the JSON object the tool expects
    /// (per its `inputSchema`).
    ///
    /// Returns `Ok` even when the tool itself reported an error
    /// (`is_error == true`) — the call completed at the protocol
    /// level. Distinguish "the call failed" (returns `Err`) from
    /// "the tool said no" (`Ok` with `is_error: true`) at the call
    /// site.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolResult, McpError> {
        let id = self.fresh_id();
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: "tools/call",
            params: json!({
                "name": name,
                "arguments": arguments,
            }),
        };
        let response = self.round_trip::<ToolsCallResult>(&request, "tools/call")?;
        let text = collect_text(&response.content);
        Ok(McpToolResult {
            text,
            is_error: response.is_error,
            structured: response.structured_content,
        })
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send one request, receive one response, validate id, decode
    /// the `result` payload as `T`. NO retry on any failure.
    fn round_trip<T>(&mut self, request: &JsonRpcRequest, label: &str) -> Result<T, McpError>
    where
        T: serde::de::DeserializeOwned,
    {
        let request_str = serde_json::to_string(request)
            .map_err(|e| McpError::Protocol(format!("serialise {label}: {e}")))?;
        self.transport.send(&request_str)?;

        let response_str = self.transport.recv()?;
        let response: JsonRpcResponse = serde_json::from_str(&response_str)
            .map_err(|e| McpError::Protocol(format!("parse {label} response: {e}")))?;
        unwrap_result::<T>(response, request.id, label)
    }
}

// ---------- helpers ----------

/// Convert a `JsonRpcResponse` into either `T` (decoded `result`) or
/// the appropriate `McpError`.
fn unwrap_result<T>(response: JsonRpcResponse, expected_id: u64, label: &str) -> Result<T, McpError>
where
    T: serde::de::DeserializeOwned,
{
    if let Some(error) = response.error {
        return Err(McpError::JsonRpc {
            code: error.code,
            message: error.message,
        });
    }
    let id = response
        .id
        .ok_or_else(|| McpError::Protocol(format!("{label}: response missing id")))?;
    let id_match = id.as_u64() == Some(expected_id);
    if !id_match {
        return Err(McpError::Protocol(format!(
            "{label}: id mismatch (expected {expected_id}, got {id})"
        )));
    }
    let result_value = response
        .result
        .ok_or_else(|| McpError::Protocol(format!("{label}: response missing result")))?;
    serde_json::from_value(result_value)
        .map_err(|e| McpError::Protocol(format!("{label}: decode result: {e}")))
}

fn into_mcp_tool(wire: WireTool) -> McpTool {
    McpTool {
        name: wire.name,
        description: wire.description,
        input_schema: wire.input_schema,
    }
}

fn collect_text(content: &[WireContent]) -> String {
    let mut buf = String::new();
    for block in content {
        if let WireContent::Text { text } = block {
            buf.push_str(text);
        }
    }
    buf
}
