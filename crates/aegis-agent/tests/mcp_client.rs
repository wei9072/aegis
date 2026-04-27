//! V3.2b — MCP client tests using `ScriptedTransport`.
//!
//! Drives the JSON-RPC protocol layer end-to-end without spawning
//! subprocesses. Verifies:
//!   - initialize handshake (request shape + initialized notification)
//!   - tools/list discovery
//!   - tools/call happy + error paths
//!   - JSON-RPC protocol violations surface as McpError (no retry)
//!   - JSON-RPC error responses surface as McpError (no retry)
//!   - id mismatch surfaces as protocol error
//!   - transport EOF / failure surfaces as McpError (no retry)
//!   - McpToolExecutor wraps the client for the conversation runtime

use aegis_agent::mcp::{McpClient, McpError, McpToolExecutor, ScriptedTransport};
use aegis_agent::tool::ToolExecutor;
use serde_json::{json, Value};

/// Build a transport pre-loaded with a successful `initialize`
/// response. Returns the transport (clone-able) so the caller can
/// keep an inspection handle.
fn transport_with_initialize() -> ScriptedTransport {
    let transport = ScriptedTransport::new();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "test-server", "version": "0.0.1" }
            }
        })
        .to_string(),
    );
    transport
}

// ---------- handshake ----------

#[test]
fn initialize_handshake_captures_server_info() {
    let transport = transport_with_initialize();
    let client = McpClient::new(Box::new(transport.clone())).unwrap();
    assert_eq!(client.server_name, "test-server");
    assert_eq!(client.server_version, "0.0.1");
    assert_eq!(client.server_protocol_version, "2025-06-18");
}

#[test]
fn initialize_sends_initialize_request_then_initialized_notification() {
    let transport = transport_with_initialize();
    let _client = McpClient::new(Box::new(transport.clone())).unwrap();

    let sends = transport.recorded_sends();
    assert_eq!(sends.len(), 2, "expected initialize + initialized");

    let init: Value = serde_json::from_str(&sends[0]).unwrap();
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["id"], 1);
    assert_eq!(init["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["params"]["clientInfo"]["name"], "aegis-agent");

    let initialized: Value = serde_json::from_str(&sends[1]).unwrap();
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["method"], "notifications/initialized");
    assert!(
        initialized.get("id").is_none() || initialized["id"].is_null(),
        "initialized must be a notification (no id)"
    );
}

#[test]
fn initialize_failure_surfaces_as_jsonrpc_error_no_retry() {
    let transport = ScriptedTransport::new();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32603, "message": "internal error" }
        })
        .to_string(),
    );

    let result = McpClient::new(Box::new(transport.clone()));
    let error = result.err().expect("expected initialize to fail");
    match error {
        McpError::JsonRpc { code, message } => {
            assert_eq!(code, -32603);
            assert_eq!(message, "internal error");
        }
        other => panic!("expected JsonRpc, got {other:?}"),
    }
    // Only one round trip — no retry.
    assert_eq!(transport.recorded_sends().len(), 1);
}

#[test]
fn initialize_with_eof_surfaces_transport_error_no_retry() {
    let transport = ScriptedTransport::new();
    transport.push_recv_err(McpError::Transport("EOF".into()));

    let result = McpClient::new(Box::new(transport.clone()));
    match result.err().unwrap() {
        McpError::Transport(message) => assert!(message.contains("EOF")),
        other => panic!("expected Transport, got {other:?}"),
    }
    assert_eq!(transport.recorded_sends().len(), 1);
}

// ---------- list_tools ----------

#[test]
fn list_tools_returns_server_tool_definitions() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echoes input",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "text": { "type": "string" } }
                        }
                    },
                    {
                        "name": "add",
                        "description": "Adds two numbers",
                        "inputSchema": { "type": "object" }
                    }
                ]
            }
        })
        .to_string(),
    );

    let mut client = McpClient::new(Box::new(transport)).unwrap();
    let tools = client.list_tools().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Echoes input");
    assert_eq!(tools[0].input_schema["type"], "object");
    assert_eq!(tools[1].name, "add");
}

// ---------- call_tool ----------

#[test]
fn call_tool_happy_path_returns_text_content() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [{ "type": "text", "text": "hello world" }],
                "isError": false
            }
        })
        .to_string(),
    );

    let mut client = McpClient::new(Box::new(transport)).unwrap();
    let result = client.call_tool("echo", json!({ "text": "hi" })).unwrap();
    assert_eq!(result.text, "hello world");
    assert!(!result.is_error);
}

#[test]
fn call_tool_concatenates_multiple_text_blocks_in_order() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [
                    { "type": "text", "text": "part 1\n" },
                    { "type": "text", "text": "part 2" }
                ],
                "isError": false
            }
        })
        .to_string(),
    );

    let mut client = McpClient::new(Box::new(transport)).unwrap();
    let result = client.call_tool("multi", json!({})).unwrap();
    assert_eq!(result.text, "part 1\npart 2");
}

#[test]
fn call_tool_with_is_error_returns_ok_with_flag_set() {
    // is_error: true is a tool-level domain failure, NOT a protocol
    // error. The MCP client returns it as Ok so the wrapper layer
    // (McpToolExecutor) can decide how to surface it.
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "content": [{ "type": "text", "text": "tool refused: bad input" }],
                "isError": true
            }
        })
        .to_string(),
    );

    let mut client = McpClient::new(Box::new(transport)).unwrap();
    let result = client.call_tool("strict", json!({})).unwrap();
    assert!(result.is_error);
    assert_eq!(result.text, "tool refused: bad input");
}

#[test]
fn call_tool_jsonrpc_error_surfaces_as_mcp_error_no_retry() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "error": { "code": -32601, "message": "unknown tool: nope" }
        })
        .to_string(),
    );

    let mut client = McpClient::new(Box::new(transport.clone())).unwrap();
    let result = client.call_tool("nope", json!({}));
    match result {
        Err(McpError::JsonRpc { code, message }) => {
            assert_eq!(code, -32601);
            assert!(message.contains("unknown tool"));
        }
        other => panic!("expected JsonRpc error, got {other:?}"),
    }
    // initialize + initialized + tools/call → 3 sends, no retry.
    assert_eq!(transport.recorded_sends().len(), 3);
}

#[test]
fn call_tool_id_mismatch_is_protocol_error_no_retry() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 999,
            "result": { "content": [], "isError": false }
        })
        .to_string(),
    );

    let mut client = McpClient::new(Box::new(transport.clone())).unwrap();
    let result = client.call_tool("anything", json!({}));
    match result {
        Err(McpError::Protocol(message)) => assert!(message.contains("id mismatch")),
        other => panic!("expected Protocol error, got {other:?}"),
    }
    assert_eq!(transport.recorded_sends().len(), 3);
}

#[test]
fn call_tool_transport_eof_surfaces_as_transport_error_no_retry() {
    let transport = transport_with_initialize();
    transport.push_recv_err(McpError::Transport("EOF on subprocess stdout".into()));

    let mut client = McpClient::new(Box::new(transport.clone())).unwrap();
    let result = client.call_tool("echo", json!({}));
    match result {
        Err(McpError::Transport(message)) => assert!(message.contains("EOF")),
        other => panic!("expected Transport error, got {other:?}"),
    }
    assert_eq!(transport.recorded_sends().len(), 3);
}

// ---------- McpToolExecutor (bridge to ToolExecutor) ----------

#[test]
fn executor_caches_tool_definitions_for_runtime() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [
                    { "name": "echo", "description": "Echoes",
                      "inputSchema": { "type": "object" } }
                ]
            }
        })
        .to_string(),
    );

    let client = McpClient::new(Box::new(transport)).unwrap();
    let executor = McpToolExecutor::new(client).unwrap();

    let defs = executor.tool_definitions();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "echo");
    assert_eq!(defs[0].description, "Echoes");
}

#[test]
fn executor_translates_is_error_into_tool_error_for_runtime() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{ "name": "strict", "description": "", "inputSchema": {} }]
            }
        })
        .to_string(),
    );
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": {
                "content": [{ "type": "text", "text": "bad input" }],
                "isError": true
            }
        })
        .to_string(),
    );

    let client = McpClient::new(Box::new(transport)).unwrap();
    let mut executor = McpToolExecutor::new(client).unwrap();

    let err = executor.execute("strict", "{}").unwrap_err();
    assert_eq!(err.message(), "bad input");
}

#[test]
fn executor_rejects_tool_not_in_cached_list() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": { "tools": [] }
        })
        .to_string(),
    );

    let client = McpClient::new(Box::new(transport)).unwrap();
    let mut executor = McpToolExecutor::new(client).unwrap();

    let err = executor.execute("nonexistent", "{}").unwrap_err();
    assert!(err.message().contains("not registered"));
}

#[test]
fn executor_invalid_json_arguments_become_tool_error_no_call() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{ "name": "echo", "description": "", "inputSchema": {} }]
            }
        })
        .to_string(),
    );

    let client = McpClient::new(Box::new(transport.clone())).unwrap();
    let mut executor = McpToolExecutor::new(client).unwrap();

    let err = executor.execute("echo", "this is not json").unwrap_err();
    assert!(err.message().contains("invalid JSON"));
    // The malformed-input rejection happens BEFORE any tools/call —
    // total sends is just initialize + initialized + tools/list.
    assert_eq!(transport.recorded_sends().len(), 3);
}

#[test]
fn executor_empty_input_string_defaults_to_empty_object() {
    let transport = transport_with_initialize();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{ "name": "noargs", "description": "", "inputSchema": {} }]
            }
        })
        .to_string(),
    );
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": {
                "content": [{ "type": "text", "text": "ran" }],
                "isError": false
            }
        })
        .to_string(),
    );

    let client = McpClient::new(Box::new(transport.clone())).unwrap();
    let mut executor = McpToolExecutor::new(client).unwrap();

    let output = executor.execute("noargs", "").unwrap();
    assert_eq!(output, "ran");

    // Inspect the tools/call payload: arguments must be {} not null.
    let sends = transport.recorded_sends();
    let call_payload: Value = serde_json::from_str(sends.last().unwrap()).unwrap();
    assert_eq!(call_payload["params"]["arguments"], json!({}));
}
