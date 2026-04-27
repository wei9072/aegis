//! V3.1b — wire `OpenAiCompatProvider` into `ConversationRuntime`
//! end-to-end with a stub HTTP backend.
//!
//! This is the integration test that confirms the provider and the
//! conversation loop actually compose: scripted Anthropic-shaped HTTP
//! responses drive an end-to-end multi-turn agent flow.

use aegis_agent::providers::{
    HttpClient, OpenAiCompatConfig, OpenAiCompatProvider, StubHttpClient,
};
use aegis_agent::testing::ScriptedToolExecutor;
use aegis_agent::{
    AgentConfig, ConversationRuntime, MessageRole, Session, StoppedReason, ToolDefinition,
};
use serde_json::json;
use std::sync::Arc;

struct StubHttpRef(Arc<StubHttpClient>);

impl HttpClient for StubHttpRef {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<aegis_agent::providers::HttpResponse, aegis_agent::providers::HttpError> {
        self.0.post(url, headers, body)
    }
}

fn cfg() -> OpenAiCompatConfig {
    OpenAiCompatConfig {
        base_url: "https://example.test/v1".into(),
        api_key: Some("sk-test".into()),
        model: "test-model".into(),
        max_tokens: 1024,
    }
}

#[test]
fn provider_plus_runtime_handles_text_then_tool_then_text() {
    let stub = Arc::new(StubHttpClient::new());
    // Round 1: assistant requests a tool call.
    stub.push_ok(
        200,
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_a",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{\"text\":\"hi\"}" }
                    }]
                }
            }]
        })
        .to_string(),
    );
    // Round 2: assistant produces a final text response.
    stub.push_ok(
        200,
        json!({
            "choices": [{
                "message": { "role": "assistant", "content": "all done" }
            }]
        })
        .to_string(),
    );

    let provider = OpenAiCompatProvider::new(cfg(), Box::new(StubHttpRef(stub.clone())));
    let tools = ScriptedToolExecutor::new().with_ok("echo", "hi");
    let tool_defs = vec![ToolDefinition::new(
        "echo",
        "Echo input",
        json!({ "type": "object" }),
    )];

    let mut rt = ConversationRuntime::new(
        Session::new(),
        provider,
        tools,
        vec!["You can use tools.".into()],
        tool_defs,
        AgentConfig {
            max_iterations_per_turn: 5,
            session_cost_budget: None,
        },
    );

    let result = rt.run_turn("please echo");

    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
    assert_eq!(result.iterations, 2);

    // Two HTTP POSTs total (one per assistant turn).
    assert_eq!(stub.recorded_requests().len(), 2);

    // Session should hold: user + assistant(tool_use) + tool_result + assistant(text).
    let messages = &rt.session().messages;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[2].role, MessageRole::Tool);
    assert_eq!(messages[3].role, MessageRole::Assistant);

    // The second HTTP request must include the tool_result message
    // — proving the provider re-serialised the tool_result block
    // correctly into the OpenAI tool-message shape.
    let second_body: serde_json::Value =
        serde_json::from_str(&stub.recorded_requests()[1].body).unwrap();
    let second_messages = second_body["messages"].as_array().unwrap();
    let tool_message = second_messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool message missing from round 2 request");
    assert_eq!(tool_message["tool_call_id"], "call_a");
    assert_eq!(tool_message["name"], "echo");
}

#[test]
fn http_error_during_runtime_terminates_with_provider_error_no_retry() {
    let stub = Arc::new(StubHttpClient::new());
    stub.push_ok(503, r#"{"error":"upstream unavailable"}"#);

    let provider = OpenAiCompatProvider::new(cfg(), Box::new(StubHttpRef(stub.clone())));
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(
        Session::new(),
        provider,
        tools,
        vec![],
        vec![],
        AgentConfig {
            max_iterations_per_turn: 5,
            session_cost_budget: None,
        },
    );

    let result = rt.run_turn("hi");

    match result.stopped_reason {
        StoppedReason::ProviderError(message) => {
            assert!(
                message.contains("HTTP 503"),
                "expected HTTP 503 in provider error, got: {message}"
            );
        }
        other => panic!("expected ProviderError, got {other:?}"),
    }
    assert_eq!(result.iterations, 1);
    // The runtime did NOT retry behind the user's back.
    assert_eq!(stub.recorded_requests().len(), 1);
}
