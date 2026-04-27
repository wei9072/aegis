//! V3.2c — Gemini provider tests.

use aegis_agent::api::{ApiClient, ApiRequest, AssistantEvent, ToolDefinition};
use aegis_agent::message::{ContentBlock, ConversationMessage, MessageRole};
use aegis_agent::providers::{GeminiConfig, GeminiProvider, HttpClient, StubHttpClient};
use serde_json::{json, Value};
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

fn provider_with_stub() -> (GeminiProvider, Arc<StubHttpClient>) {
    let stub = Arc::new(StubHttpClient::new());
    let provider = GeminiProvider::new(
        GeminiConfig {
            base_url: "https://gemini.test".into(),
            api_key: "AIza-test".into(),
            model: "gemini-test".into(),
            max_tokens: 1024,
        },
        Box::new(StubHttpRef(stub.clone())),
    );
    (provider, stub)
}

fn ok_text(text: &str) -> String {
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": text }] }
        }]
    })
    .to_string()
}

#[test]
fn endpoint_embeds_model_in_url() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    assert_eq!(
        stub.recorded_requests()[0].url,
        "https://gemini.test/v1beta/models/gemini-test:generateContent"
    );
}

#[test]
fn auth_uses_x_goog_api_key_header() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let headers = &stub.recorded_requests()[0].headers;
    let key = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-goog-api-key"))
        .expect("x-goog-api-key missing");
    assert_eq!(key.1, "AIza-test");

    // No Bearer / x-api-key (those are OpenAI / Anthropic shape).
    assert!(!headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("authorization")));
    assert!(!headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("x-api-key")));
}

#[test]
fn system_prompt_lands_in_system_instruction_field() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec!["a".into(), "b".into()],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "a\n\nb");
    assert!(body["contents"].as_array().unwrap().iter().all(|c| c["role"] != "system"));
}

#[test]
fn assistant_role_serialised_as_model() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("done"));

    let assistant = ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![ContentBlock::Text { text: "hi".into() }],
    };
    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("q"), assistant],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert_eq!(body["contents"][1]["role"], "model");
}

#[test]
fn tool_use_serialises_as_function_call_part() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("done"));

    let assistant = ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![ContentBlock::ToolUse {
            id: "ignored_by_gemini".into(),
            name: "echo".into(),
            input: r#"{"text":"hi"}"#.into(),
        }],
    };
    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![assistant],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let part = &body["contents"][0]["parts"][0];
    assert_eq!(part["functionCall"]["name"], "echo");
    assert_eq!(part["functionCall"]["args"]["text"], "hi");
}

#[test]
fn tool_result_serialises_as_function_response_user_message() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("noted"));

    let tool_msg = ConversationMessage::tool_result("ignored_by_gemini", "echo", "hi", false);
    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![tool_msg],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let content = &body["contents"][0];
    assert_eq!(content["role"], "user");
    assert_eq!(content["parts"][0]["functionResponse"]["name"], "echo");
    assert_eq!(content["parts"][0]["functionResponse"]["response"]["output"], "hi");
}

#[test]
fn tool_definition_wraps_in_function_declarations() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("ok"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![ToolDefinition::new("echo", "Echoes", json!({"type":"object"}))],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let decls = tools[0]["functionDeclarations"].as_array().unwrap();
    assert_eq!(decls[0]["name"], "echo");
    assert_eq!(decls[0]["parameters"]["type"], "object");
}

#[test]
fn text_response_parses_to_text_delta_then_stop() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text("hello"));

    let events = provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    assert_eq!(events.len(), 2);
    matches!(events[0], AssistantEvent::TextDelta(_));
    matches!(events[1], AssistantEvent::MessageStop);
}

#[test]
fn function_call_response_parses_to_tool_use_with_synthesized_id() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(
        200,
        json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "functionCall": { "name": "echo", "args": { "text": "hi" } } }
                ]}
            }]
        })
        .to_string(),
    );

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
            assert!(id.starts_with("gem_call_"));
            assert_eq!(name, "echo");
            assert_eq!(input, r#"{"text":"hi"}"#);
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

#[test]
fn http_500_is_runtime_error_no_retry() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(500, "{}");

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert!(result.unwrap_err().message().contains("HTTP 500"));
    assert_eq!(stub.recorded_requests().len(), 1);
}

#[test]
fn transport_error_no_retry() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_err("connection refused");

    let _ = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert_eq!(stub.recorded_requests().len(), 1);
}
