//! V3.3 follow-up — `AegisPredictor` unit tests using a scripted
//! `McpClient` (so no real aegis-mcp subprocess required).

use aegis_agent::aegis_predict::AegisPredictor;
use aegis_agent::mcp::{McpClient, ScriptedTransport};
use aegis_agent::predict::{PreToolUsePredictor, PredictVerdict};
use serde_json::{json, Value};

/// Build an `McpClient` over a `ScriptedTransport` already loaded
/// with a successful `initialize` response.
fn make_client(extra_recv: Vec<String>) -> McpClient {
    let transport = ScriptedTransport::new();
    transport.push_recv(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": { "name": "stub", "version": "0" }
            }
        })
        .to_string(),
    );
    for r in extra_recv {
        transport.push_recv(r);
    }
    McpClient::new(Box::new(transport)).unwrap()
}

fn validate_change_response(decision: &str) -> String {
    let verdict = json!({
        "decision": decision,
        "reasons": if decision == "BLOCK" {
            json!([{ "layer": "ring0_5", "decision": "block", "reason": "fan_out_increase" }])
        } else {
            json!([])
        },
    });
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": {
            "content": [{ "type": "text", "text": verdict.to_string() }],
            "isError": false
        }
    })
    .to_string()
}

#[test]
fn unwatched_tool_passes_through_without_mcp_call() {
    let client = make_client(vec![]);
    let mut predictor = AegisPredictor::new(client);

    // The scripted transport has NO tools/call response queued —
    // if the predictor hit the MCP server, it would fail. Allow
    // verdict on an unwatched tool name proves no call was made.
    let verdict = predictor.predict("read_file", r#"{"path":"x.py"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);
}

#[test]
fn watched_tool_block_decision_becomes_block_verdict() {
    let client = make_client(vec![validate_change_response("BLOCK")]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict(
        "Edit",
        r#"{"path":"trivial.py","new_content":"def f(): pass"}"#,
    );
    match verdict {
        PredictVerdict::Block { reason } => {
            assert!(reason.contains("BLOCK"));
            assert!(reason.contains("Edit"));
            assert!(reason.contains("trivial.py"));
        }
        PredictVerdict::Allow => panic!("expected Block on BLOCK decision"),
    }
}

#[test]
fn watched_tool_pass_decision_becomes_allow_verdict() {
    let client = make_client(vec![validate_change_response("PASS")]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict("Write", r#"{"path":"a.py","new_content":"x=1"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);
}

#[test]
fn malformed_input_falls_open_with_diagnostic() {
    let client = make_client(vec![]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict("Edit", "this is not json");
    assert_eq!(verdict, PredictVerdict::Allow);
    assert!(predictor
        .last_diagnostic
        .as_ref()
        .unwrap()
        .contains("not JSON"));
}

#[test]
fn missing_path_field_falls_open_with_diagnostic() {
    let client = make_client(vec![]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict("Edit", r#"{"content":"x"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);
    assert!(predictor
        .last_diagnostic
        .as_ref()
        .unwrap()
        .contains("no 'path' field"));
}

#[test]
fn missing_content_field_falls_open_with_diagnostic() {
    let client = make_client(vec![]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict("Edit", r#"{"path":"x.py"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);
    assert!(predictor
        .last_diagnostic
        .as_ref()
        .unwrap()
        .contains("no recognised content field"));
}

#[test]
fn alternative_content_field_names_are_accepted() {
    // Anthropic-style "new_string"
    let client = make_client(vec![validate_change_response("PASS")]);
    let mut predictor = AegisPredictor::new(client);
    let verdict = predictor.predict("Edit", r#"{"path":"a.py","new_string":"x=1"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);

    // Generic "content"
    let client = make_client(vec![validate_change_response("PASS")]);
    let mut predictor = AegisPredictor::new(client);
    let verdict = predictor.predict("Edit", r#"{"path":"a.py","content":"x=1"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);
}

#[test]
fn watch_tool_extends_recognised_set() {
    let client = make_client(vec![validate_change_response("BLOCK")]);
    let mut predictor = AegisPredictor::new(client).watch_tool("custom_writer");

    let verdict = predictor.predict("custom_writer", r#"{"path":"x.py","new_content":"y"}"#);
    assert!(matches!(verdict, PredictVerdict::Block { .. }));
}

#[test]
fn mcp_call_failure_falls_open_with_diagnostic() {
    let client = make_client(vec![
        // Send a malformed response — McpClient will surface protocol error
        "not json at all".to_string(),
    ]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict("Edit", r#"{"path":"a.py","new_content":"x"}"#);
    assert_eq!(verdict, PredictVerdict::Allow);
    assert!(predictor.last_diagnostic.as_ref().unwrap().contains("MCP"));
}

#[test]
fn malformed_verdict_payload_falls_open() {
    // Server returned a tools/call response but the inner text isn't JSON.
    let response = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "result": {
            "content": [{ "type": "text", "text": "not a verdict at all" }],
            "isError": false
        }
    })
    .to_string();
    let client = make_client(vec![response]);
    let mut predictor = AegisPredictor::new(client);

    let verdict = predictor.predict("Edit", r#"{"path":"a.py","new_content":"x"}"#);
    // Verdict text not JSON → fall open.
    assert_eq!(verdict, PredictVerdict::Allow);
}

// Sanity-only: verify Value is the right import (silences unused
// warning if test config changes).
#[test]
fn value_import_referenced() {
    let _: Value = json!({});
}
