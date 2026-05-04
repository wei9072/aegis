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

use aegis_core::attest::{append_attestation_log, attest};
use aegis_core::validate::{validate_change, validate_change_with_workspace};
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
    let single_file_tool = json!({
        "name": "validate_change",
        "description": "Fast single-file gate. Run Aegis Ring 0 (syntax) + \
                        Ring 0.5 (structural signals + cost regression) + \
                        Ring 0.7 (security anti-patterns) on a proposed file \
                        write. Returns the decision without applying the \
                        change. Pure observation — never coaches the agent. \
                        Use this when the change is contained to one file or \
                        when speed matters more than cross-file safety.",
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
    let workspace_tool = json!({
        "name": "validate_change_with_workspace",
        "description": "Workspace-aware gate (Ring 0 + 0.5 + 0.7 + R2). Adds \
                        cross-file checks on top of validate_change: detects \
                        when a change introduces a module import cycle, or \
                        deletes a public symbol that other files in the \
                        workspace still reference. Slower than validate_change \
                        because it walks the workspace tree; prefer this when \
                        the change touches a public API or shared module.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or workspace-relative path the \
                                    agent intends to write."
                },
                "new_content": {
                    "type": "string",
                    "description": "Full file contents the agent intends to write."
                },
                "old_content": {
                    "type": "string",
                    "description": "Optional baseline for cost-aware regression."
                },
                "workspace_root": {
                    "type": "string",
                    "description": "Absolute path to the project root. Used to \
                                    build a one-shot workspace index for cycle \
                                    detection and public-symbol reference \
                                    tracking."
                }
            },
            "required": ["path", "new_content", "workspace_root"]
        }
    });
    let attest_tool = json!({
        "name": "attest_path",
        "description": "Post-write attestation. Reads on-disk content of \
                        `path` and runs absolute checks (Ring 0 syntax + \
                        Ring 0.7 security + optional Ring R2 cycle). Use \
                        from PostToolUse hooks / CI / after any write that \
                        bypasses the pre-write gate. Writes the verdict to \
                        `<workspace_root>/.aegis/attestations.jsonl` for \
                        audit when workspace_root is provided.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path of the file to attest."
                },
                "workspace_root": {
                    "type": "string",
                    "description": "Optional. Enables Ring R2 cycle detection \
                                    and JSONL audit log."
                }
            },
            "required": ["path"]
        }
    });
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(json!({
            "tools": [single_file_tool, workspace_tool, attest_tool]
        })),
        error: None,
    }
}

fn handle_tools_call(id: Value, params: &Value) -> JsonRpcResponse {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(
        name,
        "validate_change" | "validate_change_with_workspace" | "attest_path"
    ) {
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

    let verdict = if name == "attest_path" {
        let workspace_root = args
            .get("workspace_root")
            .and_then(|v| v.as_str())
            .map(String::from);
        let v = attest(&path, workspace_root.as_deref());
        if let Some(ref ws) = workspace_root {
            // Best-effort log append; never fail the tool call on it.
            let _ = append_attestation_log(ws, &v);
        }
        serde_json::to_value(&v).unwrap_or(Value::Null)
    } else {
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

        if name == "validate_change_with_workspace" {
            let workspace_root = match args.get("workspace_root").and_then(|v| v.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    return tool_error(id, "missing required argument 'workspace_root'");
                }
            };
            validate_change_with_workspace(
                &path,
                &new_content,
                old_content.as_deref(),
                &workspace_root,
            )
            .to_value()
        } else {
            validate_change(&path, &new_content, old_content.as_deref()).to_value()
        }
    };
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
