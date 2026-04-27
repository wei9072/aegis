//! V3.1b — OpenAI-compat provider tests.
//!
//! Drives the provider end-to-end against a `StubHttpClient`:
//!   - request body shape (system / user / assistant / tool messages)
//!   - tool definition serialisation
//!   - auth header presence + absence
//!   - response parsing (text-only, tool-call-only, mixed)
//!   - HTTP 4xx / 5xx surfaces as `RuntimeError` (NO retry)
//!   - transport error surfaces as `RuntimeError` (NO retry)
//!   - missing-key local backend (Ollama-style) works without auth
//!
//! These tests do NOT hit the network. For a live-API smoke check,
//! see `tests/openai_compat_smoke.rs` (env-gated, `#[ignore]`).

use aegis_agent::api::{ApiClient, ApiRequest, AssistantEvent, ToolDefinition};
use aegis_agent::message::{ContentBlock, ConversationMessage, MessageRole};
use aegis_agent::providers::{
    HttpClient, OpenAiCompatConfig, OpenAiCompatProvider, StubHttpClient,
};
use serde_json::{json, Value};

fn provider_with_stub(
    config: OpenAiCompatConfig,
) -> (OpenAiCompatProvider, std::sync::Arc<StubHttpClient>) {
    let stub = std::sync::Arc::new(StubHttpClient::new());
    let stub_for_provider = stub.clone();
    let http: Box<dyn HttpClient> = Box::new(StubHttpRef(stub_for_provider));
    (OpenAiCompatProvider::new(config, http), stub)
}

/// Adapter so we can hand the provider a `Box<dyn HttpClient>` while
/// still asserting on the same StubHttpClient instance from outside.
struct StubHttpRef(std::sync::Arc<StubHttpClient>);

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

fn cfg(api_key: Option<&str>) -> OpenAiCompatConfig {
    OpenAiCompatConfig {
        base_url: "https://example.test/v1".into(),
        api_key: api_key.map(String::from),
        model: "test-model".into(),
        max_tokens: 1024,
    }
}

fn ok_text_response(text: &str) -> String {
    json!({
        "choices": [
            { "message": { "role": "assistant", "content": text } }
        ]
    })
    .to_string()
}

fn ok_tool_call_response(id: &str, name: &str, args: &str) -> String {
    json!({
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": args }
                        }
                    ]
                }
            }
        ]
    })
    .to_string()
}

// ---------- request shape ----------

#[test]
fn endpoint_is_chat_completions_appended_to_base_url() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("hi"));

    let req = ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hello")],
        tools: vec![],
    };
    provider.stream(req).unwrap();

    let recorded = stub.recorded_requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].url, "https://example.test/v1/chat/completions");
}

#[test]
fn auth_header_present_when_api_key_provided() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-secret")));
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let headers = &stub.recorded_requests()[0].headers;
    let auth = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .expect("authorization header missing");
    assert_eq!(auth.1, "Bearer sk-secret");
}

#[test]
fn auth_header_absent_for_local_backend_without_key() {
    // Mirrors Ollama / llama.cpp / LMStudio config — no api_key, no
    // auth header. The backend must accept the request anyway.
    let (mut provider, stub) = provider_with_stub(cfg(None));
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let headers = &stub.recorded_requests()[0].headers;
    let auth_present = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
    assert!(!auth_present, "expected NO authorization header");
}

#[test]
fn request_body_includes_model_and_max_tokens() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_tokens"], 1024);
}

#[test]
fn request_body_collapses_system_prompt_into_one_system_message() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec!["line one".into(), "line two".into()],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "line one\n\nline two");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "hi");
}

#[test]
fn request_body_serialises_assistant_tool_use_as_tool_calls() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("done"));

    let assistant_message = ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![
            ContentBlock::Text {
                text: "let me check".into(),
            },
            ContentBlock::ToolUse {
                id: "call_42".into(),
                name: "echo".into(),
                input: r#"{"text":"hi"}"#.into(),
            },
        ],
    };
    let tool_result = ConversationMessage::tool_result("call_42", "echo", "hi", false);

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![
                ConversationMessage::user_text("please echo"),
                assistant_message,
                tool_result,
            ],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let messages = body["messages"].as_array().unwrap();

    // user, assistant, tool
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");

    let assistant = &messages[1];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "let me check");
    let calls = assistant["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["id"], "call_42");
    assert_eq!(calls[0]["type"], "function");
    assert_eq!(calls[0]["function"]["name"], "echo");
    assert_eq!(calls[0]["function"]["arguments"], r#"{"text":"hi"}"#);

    let tool = &messages[2];
    assert_eq!(tool["role"], "tool");
    assert_eq!(tool["tool_call_id"], "call_42");
    assert_eq!(tool["name"], "echo");
    assert_eq!(tool["content"], "hi");
}

#[test]
fn tool_definition_serialises_to_openai_function_shape() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("ok"));

    let tool = ToolDefinition::new(
        "echo",
        "Echo the input text",
        json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        }),
    );

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![tool],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "echo");
    assert_eq!(tools[0]["function"]["description"], "Echo the input text");
    assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
    assert!(tools[0]["function"]["parameters"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "text"));
}

#[test]
fn tools_omitted_from_payload_when_none_supplied() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert!(
        body.get("tools").is_none(),
        "tools field should be omitted when empty"
    );
}

#[test]
fn tool_result_with_is_error_marks_output_for_model() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("noted"));

    let tool_msg = ConversationMessage::tool_result("c1", "broken", "boom", true);
    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![tool_msg],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "tool");
    assert_eq!(messages[0]["content"], "[error] boom");
}

// ---------- response parsing ----------

#[test]
fn text_response_parses_into_text_delta_then_stop() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_text_response("hello world"));

    let events = provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    assert_eq!(events.len(), 2);
    match &events[0] {
        AssistantEvent::TextDelta(text) => assert_eq!(text, "hello world"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
    matches!(events[1], AssistantEvent::MessageStop);
}

#[test]
fn tool_call_response_parses_into_tool_use_then_stop() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, ok_tool_call_response("call_99", "echo", r#"{"x":1}"#));

    let events = provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("call echo")],
            tools: vec![],
        })
        .unwrap();

    assert_eq!(events.len(), 2);
    match &events[0] {
        AssistantEvent::ToolUse { id, name, input } => {
            assert_eq!(id, "call_99");
            assert_eq!(name, "echo");
            assert_eq!(input, r#"{"x":1}"#);
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
    matches!(events[1], AssistantEvent::MessageStop);
}

#[test]
fn mixed_text_and_tool_call_response_parses_both_in_order() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(
        200,
        json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "checking now",
                        "tool_calls": [
                            { "id": "c", "type": "function",
                              "function": { "name": "x", "arguments": "{}" } }
                        ]
                    }
                }
            ]
        })
        .to_string(),
    );

    let events = provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    assert_eq!(events.len(), 3);
    matches!(events[0], AssistantEvent::TextDelta(_));
    matches!(events[1], AssistantEvent::ToolUse { .. });
    matches!(events[2], AssistantEvent::MessageStop);
}

#[test]
fn empty_response_is_runtime_error_not_silent_pass() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(
        200,
        json!({
            "choices": [
                { "message": { "role": "assistant", "content": null } }
            ]
        })
        .to_string(),
    );

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert!(result.is_err(), "expected RuntimeError on empty response");
}

#[test]
fn zero_choices_is_runtime_error() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(200, json!({ "choices": [] }).to_string());

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert!(result.is_err());
}

// ---------- HTTP error handling — NO retry ----------

#[test]
fn http_500_is_runtime_error_with_status_no_retry() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(500, r#"{"error":"internal"}"#);

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    let err = result.unwrap_err();
    assert!(
        err.message().contains("HTTP 500"),
        "expected HTTP 500 in error: {}",
        err.message()
    );
    // Confirm the provider made exactly ONE HTTP call — no auto-retry.
    assert_eq!(stub.recorded_requests().len(), 1);
}

#[test]
fn http_429_is_runtime_error_no_retry() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_ok(429, r#"{"error":"rate limited"}"#);

    let _ = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert_eq!(
        stub.recorded_requests().len(),
        1,
        "rate-limit must NOT trigger an automatic retry"
    );
}

#[test]
fn http_401_is_runtime_error_no_fallback_to_anonymous() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("bad-key")));
    stub.push_ok(401, r#"{"error":"invalid api key"}"#);

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert!(result.is_err());
    // Critical: provider does NOT then retry without the auth header
    // ("maybe this works without auth?"). One call, one error.
    assert_eq!(stub.recorded_requests().len(), 1);
}

#[test]
fn transport_error_is_runtime_error_no_retry() {
    let (mut provider, stub) = provider_with_stub(cfg(Some("sk-x")));
    stub.push_err("dns lookup failed");

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    let err = result.unwrap_err();
    assert!(err.message().contains("dns lookup failed"));
    assert_eq!(stub.recorded_requests().len(), 1);
}
