//! `aegis-mcp` — V2 MCP server.
//!
//! Hand-rolled JSON-RPC 2.0 over stdio (MCP transport). One tool:
//! `validate_file(path, new_content, old_content?, workspace_root?)`.
//! Returns a flat `findings[]` array. No decision, no severity — the
//! consuming LLM agent decides what to do with each finding.
//!
//! When `workspace_root` is supplied, Ring R2 (cross-file) findings
//! are added to the result: cycle introduction, public-symbol-removed
//! with caller list, file_role with z-scores. The workspace index is
//! built lazily on first call (via aegis-core's mtime cache) and
//! reused across subsequent calls — no separate "scan" command
//! needed; bootstrap happens transparently.
//!
//! Why hand-rolled JSON-RPC: avoids dragging in `rmcp` for a server
//! that exposes a single tool. Spec compliance verified against
//! Anthropic's MCP client integration tests in V0.x.

use std::io::{self, BufRead, Write};

use aegis_core::findings::{
    gather_findings, gather_findings_with_workspace, FINDINGS_SCHEMA_VERSION,
};
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
            None => continue,
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
    let validate_file = json!({
        "name": "validate_file",
        "description": "Run aegis findings on a proposed file write. Returns a \
                        flat findings[] list — no decision, no severity, no \
                        verdict. Each finding is a fact (syntax error, signal \
                        delta, security pattern match, cross-file cycle, public \
                        symbol removal, file role with z-scores). The consuming \
                        agent decides which to act on. \
                        \
                        Layer 1 findings (Syntax, Signal, Security) are always \
                        produced. Pass `workspace_root` to additionally produce \
                        Layer 2 (Workspace) findings — cycle detection, broken \
                        callers, file role classification. The workspace index \
                        is built lazily on first call and cached across \
                        subsequent calls (mtime-aware). \
                        \
                        Pass `old_content` to get value_before / value_after / \
                        delta on Signal findings, otherwise signals report \
                        absolute counts only.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path the agent intends to write. Used as \
                                    the filename for syntax/structural analysis \
                                    and as the resolution root for relative \
                                    imports. No side effects to disk."
                },
                "new_content": {
                    "type": "string",
                    "description": "Full file contents the agent intends to write."
                },
                "old_content": {
                    "type": "string",
                    "description": "Optional. The file's previous contents. When \
                                    supplied, Signal findings include value_before \
                                    / value_after / delta in their context."
                },
                "workspace_root": {
                    "type": "string",
                    "description": "Optional. Absolute path to the project root. \
                                    When supplied, adds Workspace-kind findings \
                                    (cycle_introduced, public_symbol_removed, \
                                    file_role with z-scores). The workspace \
                                    index is built lazily on first call."
                }
            },
            "required": ["path", "new_content"]
        }
    });
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({
            "tools": [validate_file]
        })),
        error: None,
    }
}

fn handle_tools_call(id: Value, params: &Value) -> JsonRpcResponse {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name != "validate_file" {
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
    let workspace_root = args
        .get("workspace_root")
        .and_then(|v| v.as_str())
        .map(String::from);

    let findings = if let Some(ws) = workspace_root.as_deref() {
        gather_findings_with_workspace(&path, &new_content, old_content.as_deref(), ws)
    } else {
        gather_findings(&path, &new_content, old_content.as_deref())
    };

    let payload = json!({
        "schema_version": FINDINGS_SCHEMA_VERSION,
        "findings": findings,
    });

    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&payload).unwrap_or_default(),
            }],
            "structuredContent": payload,
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
