//! JSON-RPC 2.0 + MCP wire types.
//!
//! Kept minimal for V3.2b — only `initialize`, `tools/list`,
//! `tools/call`. Extension methods (resources, prompts, sampling)
//! land when a real consumer needs them.
//!
//! Reference: <https://modelcontextprotocol.io/specification>

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP protocol version this client speaks. Matches what `aegis-mcp`
/// declares (see `crates/aegis-mcp/src/main.rs::PROTOCOL_VERSION`).
pub const CLIENT_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Serialize)]
pub struct JsonRpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcNotification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: Value,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: String,
    #[serde(rename = "serverInfo", default)]
    pub server_info: Option<ServerInfo>,
    #[serde(default)]
    pub capabilities: Value,
}

#[derive(Debug, Deserialize, Default)]
pub struct ServerInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct ToolsListResult {
    #[serde(default)]
    pub tools: Vec<WireTool>,
}

#[derive(Debug, Deserialize)]
pub struct WireTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

#[derive(Debug, Deserialize)]
pub struct ToolsCallResult {
    /// `content[]` — typically `[{"type":"text","text":"..."}]`.
    /// MCP also supports `image` / `resource` content types; for
    /// V3.2b we surface only `text` blocks (others become an empty
    /// string with a diagnostic note).
    #[serde(default)]
    pub content: Vec<WireContent>,
    /// Server-side claim of error. Distinct from JSON-RPC `error`
    /// (which means the protocol-level call failed). `isError: true`
    /// means the tool ran but reported a domain-level failure.
    #[serde(rename = "isError", default)]
    pub is_error: bool,
    /// Optional structured payload (some servers — including
    /// aegis-mcp — emit this in addition to `content[]` for
    /// machine-friendly consumption). Pass through opaquely.
    #[serde(rename = "structuredContent", default)]
    pub structured_content: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContent {
    Text {
        text: String,
    },
    /// Other content types (`image`, `resource`, etc.) are accepted
    /// but their bodies are dropped — V3.2b only surfaces text.
    /// The presence is recorded so callers can tell the server
    /// returned non-text content.
    #[serde(other)]
    Unsupported,
}
