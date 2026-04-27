//! `aegis-mcp` — V1.9 / V1.10 Rust MCP server.
//!
//! Hand-rolled JSON-RPC 2.0 over stdio (MCP transport). One tool:
//! `validate_change(path, new_content, old_content?)` — runs Ring 0
//! syntax check + Ring 0.5 signal extraction + cost-aware regression
//! detection on a proposed file write. Returns `{decision, reasons,
//! signals_after, signals_before?, regression_detail?}`.
//!
//! Mirrors the V0.x Python `aegis_mcp/server.py` contract pinned in
//! `docs/integrations/mcp_design.md`. Intentionally narrow surface
//! (no `validate_diff`, no `get_signals`, no `retry`/`hint`/`explain`
//! tools — see post_launch_discipline.md framing).
//!
//! Why hand-rolled JSON-RPC: avoids dragging in `rmcp` for a server
//! that exposes a single tool. ~250 LOC vs. ~1MB of dependency
//! bloat. Spec compliance verified against Anthropic's MCP client
//! integration tests in V0.x.

use std::io::{self, BufRead, Write};

use aegis_core::validate::validate_change;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "aegis";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                    }),
                };
                writeln!(out, "{}", serde_json::to_string(&resp)?)?;
                out.flush()?;
                continue;
            }
        };
        if req.jsonrpc != "2.0" {
            // Not a fatal error per JSON-RPC; we just respond if id present.
        }
        // Notifications (no id) are silent.
        let id = match req.id.clone() {
            Some(v) => v,
            None => {
                // Process notification methods (initialized) silently.
                continue;
            }
        };
        let response = match req.method.as_str() {
            "initialize" => handle_initialize(id),
            "tools/list" => handle_tools_list(id),
            "tools/call" => handle_tools_call(id, &req.params),
            "ping" => JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({})),
                error: None,
            },
            other => JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("method not found: {other}"),
                }),
            },
        };
        writeln!(out, "{}", serde_json::to_string(&response)?)?;
        out.flush()?;
    }
    Ok(())
}

fn handle_initialize(id: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            }
        })),
        error: None,
    }
}

fn handle_tools_list(id: Value) -> JsonRpcResponse {
    let tool = json!({
        "name": "validate_change",
        "description": "Run Aegis Ring 0 + structural-signal extraction + \
                        cost-aware regression detection on a proposed file \
                        write. Returns the decision verdict without applying \
                        the change. Pure observation — never coaches the \
                        agent (post_launch_discipline.md).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path the agent intends to write (used as \
                                    filename for syntax/structural analysis \
                                    only — no side effects to disk)."
                },
                "new_content": {
                    "type": "string",
                    "description": "Full file contents the agent intends to write."
                },
                "old_content": {
                    "type": "string",
                    "description": "Optional. If provided, enables cost-aware \
                                    regression detection by comparing structural \
                                    signal totals before vs after."
                }
            },
            "required": ["path", "new_content"]
        }
    });
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({ "tools": [tool] })),
        error: None,
    }
}

fn handle_tools_call(id: Value, params: &Value) -> JsonRpcResponse {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name != "validate_change" {
        return JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("unknown tool: {name}"),
            }),
        };
    }
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => {
            return tool_error(id, "missing required argument 'path'");
        }
    };
    let new_content = match args.get("new_content").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            return tool_error(id, "missing required argument 'new_content'");
        }
    };
    let old_content = args
        .get("old_content")
        .and_then(|v| v.as_str())
        .map(String::from);

    let verdict = validate_change(&path, &new_content, old_content.as_deref()).to_value();
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&verdict).unwrap_or_default(),
            }],
            "structuredContent": verdict,
            "isError": false
        })),
        error: None,
    }
}

fn tool_error(id: Value, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: message.to_string(),
        }),
    }
}
