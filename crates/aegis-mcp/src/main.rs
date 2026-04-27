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
use std::path::Path;

use aegis_core::ast::registry::LanguageRegistry;
use aegis_core::enforcement::check_syntax_native;
use aegis_core::signal_layer_pyapi::extract_signals_native;
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

    let verdict = validate_change(&path, &new_content, old_content.as_deref());
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

/// Mirror of the V0.x Python `validate_change` tool. Returns a JSON
/// `Value` with `decision`, `reasons`, `signals_after`, optionally
/// `signals_before` + `regression_detail`.
fn validate_change(path: &str, new_content: &str, old_content: Option<&str>) -> Value {
    let mut reasons: Vec<Value> = Vec::new();
    let suffix = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_else(|| ".py".to_string());

    let supported_exts = LanguageRegistry::global().extensions();
    if !supported_exts.contains(&suffix.as_str()) {
        return json!({
            "decision": "BLOCK",
            "reasons": [{
                "layer": "ring0",
                "decision": "block",
                "reason": "unsupported_extension",
                "detail": format!("no language adapter for {suffix:?}; \
                                   supported: {:?}", supported_exts),
            }],
            "signals_after": {},
        });
    }

    // Write to temp file for the existing file-based APIs.
    let tmp_new = match write_temp(&suffix, new_content) {
        Ok(p) => p,
        Err(e) => {
            return json!({
                "decision": "BLOCK",
                "reasons": [{
                    "layer": "ring0",
                    "decision": "block",
                    "reason": "tempfile_error",
                    "detail": e,
                }],
                "signals_after": {},
            });
        }
    };

    // Ring 0 syntax check.
    if let Ok(violations) = check_syntax_native(&tmp_new) {
        for v in violations {
            reasons.push(json!({
                "layer": "ring0",
                "decision": "block",
                "reason": "ring0_violation",
                "detail": v,
            }));
        }
    }

    let new_sigs = match extract_signals_native(&tmp_new) {
        Ok(v) => v,
        Err(e) => {
            cleanup(&tmp_new);
            return json!({
                "decision": "BLOCK",
                "reasons": [{
                    "layer": "ring0_5",
                    "decision": "block",
                    "reason": "signal_extraction_failed",
                    "detail": e,
                }],
                "signals_after": {},
            });
        }
    };

    let mut signals_after: serde_json::Map<String, Value> = serde_json::Map::new();
    for s in &new_sigs {
        signals_after
            .entry(s.name.clone())
            .and_modify(|v| {
                let cur = v.as_f64().unwrap_or(0.0);
                *v = json!(cur + s.value);
            })
            .or_insert(json!(s.value));
    }

    let mut result = json!({
        "signals_after": signals_after,
        "reasons": reasons,
    });

    if let Some(old) = old_content {
        if let Ok(old_path) = write_temp(&suffix, old) {
            let old_sigs = extract_signals_native(&old_path).unwrap_or_default();
            cleanup(&old_path);
            let mut signals_before: serde_json::Map<String, Value> =
                serde_json::Map::new();
            for s in &old_sigs {
                signals_before
                    .entry(s.name.clone())
                    .and_modify(|v| {
                        let cur = v.as_f64().unwrap_or(0.0);
                        *v = json!(cur + s.value);
                    })
                    .or_insert(json!(s.value));
            }
            result["signals_before"] = Value::Object(signals_before.clone());

            let cost_after: f64 = new_sigs.iter().map(|s| s.value).sum();
            let cost_before: f64 = old_sigs.iter().map(|s| s.value).sum();
            if cost_after > cost_before {
                let mut growers: serde_json::Map<String, Value> = serde_json::Map::new();
                let keys: std::collections::BTreeSet<String> = signals_after
                    .keys()
                    .chain(signals_before.keys())
                    .cloned()
                    .collect();
                for key in keys {
                    let a = signals_after
                        .get(&key)
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let b = signals_before
                        .get(&key)
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    if a > b {
                        let delta = ((a - b) * 10_000.0).round() / 10_000.0;
                        growers.insert(key, json!(delta));
                    }
                }
                result["regression_detail"] = Value::Object(growers.clone());
                let reasons_array = result["reasons"].as_array_mut().unwrap();
                reasons_array.push(json!({
                    "layer": "regression",
                    "decision": "block",
                    "reason": "cost_increased",
                    "detail": format!(
                        "total cost {cost_before:.0} → {cost_after:.0}; growers: {:?}",
                        growers
                    ),
                }));
            }
        }
    }

    cleanup(&tmp_new);

    let reasons_now = result["reasons"].as_array().cloned().unwrap_or_default();
    let any_block = reasons_now
        .iter()
        .any(|r| r.get("decision").and_then(|d| d.as_str()) == Some("block"));
    let any_warn = reasons_now
        .iter()
        .any(|r| r.get("decision").and_then(|d| d.as_str()) == Some("warn"));
    let decision = if any_block {
        "BLOCK"
    } else if any_warn {
        "WARN"
    } else {
        "PASS"
    };
    result["decision"] = json!(decision);
    result
}

fn write_temp(suffix: &str, content: &str) -> Result<String, String> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("aegis-mcp-{pid}-{ts}{suffix}"));
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
}
