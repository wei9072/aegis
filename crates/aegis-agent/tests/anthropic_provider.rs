//! V3.2a — Anthropic Messages API provider tests.
//!
//! Verifies Anthropic's wire shape end-to-end against StubHttpClient:
//!   - Endpoint construction (`/v1/messages` appended to base_url)
//!   - Auth headers (`x-api-key` + `anthropic-version`, NOT Bearer)
//!   - System prompt → top-level `system` field (not message)
//!   - Conversation messages → role + content blocks
//!   - tool_result blocks → user-role messages (per Anthropic shape)
//!   - tool_use input round-trips: string → JSON object → string
//!   - Tool definitions (flat shape, not function-wrapped)
//!   - Response content blocks → AssistantEvent stream
//!   - Thinking blocks silently dropped (V3.2a)
//!   - HTTP errors → RuntimeError with NO retry

use aegis_agent::api::{ApiClient, ApiRequest, AssistantEvent, ToolDefinition};
use aegis_agent::message::{ContentBlock, ConversationMessage, MessageRole};
use aegis_agent::providers::{
    AnthropicConfig, AnthropicProvider, HttpClient, StubHttpClient,
};
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

fn provider_with_stub() -> (AnthropicProvider, Arc<StubHttpClient>) {
    let stub = Arc::new(StubHttpClient::new());
    let stub_for_provider = stub.clone();
    let provider = AnthropicProvider::new(
        AnthropicConfig {
            base_url: "https://anthropic.test".into(),
            api_key: "sk-ant-test".into(),
            model: "claude-test".into(),
            max_tokens: 1024,
            anthropic_version: "2023-06-01".into(),
        },
        Box::new(StubHttpRef(stub_for_provider)),
    );
    (provider, stub)
}

fn ok_text_response(text: &str) -> String {
    json!({
        "id": "msg_x",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": text }
        ],
        "stop_reason": "end_turn"
    })
    .to_string()
}

fn ok_tool_use_response(id: &str, name: &str, input: Value) -> String {
    json!({
        "id": "msg_x",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "tool_use", "id": id, "name": name, "input": input }
        ],
        "stop_reason": "tool_use"
    })
    .to_string()
}

// ---------- request shape ----------

#[test]
fn endpoint_is_v1_messages_under_base_url() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let recorded = stub.recorded_requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].url, "https://anthropic.test/v1/messages");
}

#[test]
fn auth_headers_use_x_api_key_not_bearer() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let headers = &stub.recorded_requests()[0].headers;
    let api_key = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
        .expect("x-api-key header missing");
    assert_eq!(api_key.1, "sk-ant-test");

    let version = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
        .expect("anthropic-version header missing");
    assert_eq!(version.1, "2023-06-01");

    // Critically: NO Authorization: Bearer header (that's OpenAI-shape).
    let bearer_present = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
    assert!(
        !bearer_present,
        "Anthropic must NOT use Authorization: Bearer header"
    );
}

#[test]
fn system_prompt_lands_in_top_level_field_not_messages() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec!["line a".into(), "line b".into()],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert_eq!(body["system"], "line a\n\nline b");

    // The messages array must NOT contain a system-role message.
    let messages = body["messages"].as_array().unwrap();
    assert!(
        messages.iter().all(|m| m["role"] != "system"),
        "system prompt must not appear as a message — found: {messages:?}"
    );
}

#[test]
fn assistant_message_with_text_and_tool_use_serialises_into_content_blocks() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("done"));

    let assistant_message = ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![
            ContentBlock::Text {
                text: "let me check".into(),
            },
            ContentBlock::ToolUse {
                id: "toolu_42".into(),
                name: "echo".into(),
                input: r#"{"text":"hi"}"#.into(),
            },
        ],
    };
    let tool_result = ConversationMessage::tool_result("toolu_42", "echo", "hi", false);

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

    // user, assistant (with text + tool_use blocks), user (with tool_result block)
    assert_eq!(messages.len(), 3);

    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["type"], "text");
    assert_eq!(messages[0]["content"][0]["text"], "please echo");

    assert_eq!(messages[1]["role"], "assistant");
    let asst_blocks = messages[1]["content"].as_array().unwrap();
    assert_eq!(asst_blocks.len(), 2);
    assert_eq!(asst_blocks[0]["type"], "text");
    assert_eq!(asst_blocks[0]["text"], "let me check");
    assert_eq!(asst_blocks[1]["type"], "tool_use");
    assert_eq!(asst_blocks[1]["id"], "toolu_42");
    assert_eq!(asst_blocks[1]["name"], "echo");
    // Critically: input is a JSON OBJECT not a string.
    assert!(asst_blocks[1]["input"].is_object());
    assert_eq!(asst_blocks[1]["input"]["text"], "hi");

    // Tool result: user-role message with tool_result content block.
    assert_eq!(messages[2]["role"], "user");
    let tr_blocks = messages[2]["content"].as_array().unwrap();
    assert_eq!(tr_blocks[0]["type"], "tool_result");
    assert_eq!(tr_blocks[0]["tool_use_id"], "toolu_42");
    assert_eq!(tr_blocks[0]["content"][0]["type"], "text");
    assert_eq!(tr_blocks[0]["content"][0]["text"], "hi");
}

#[test]
fn tool_result_with_is_error_sets_anthropic_is_error_field() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("noted"));

    let tool_msg = ConversationMessage::tool_result("toolu_x", "broken", "boom", true);

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![tool_msg],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    let tr_block = &body["messages"][0]["content"][0];
    assert_eq!(tr_block["type"], "tool_result");
    assert_eq!(tr_block["is_error"], true);
    assert_eq!(tr_block["content"][0]["text"], "boom");
}

#[test]
fn tool_definition_serialises_flat_not_function_wrapped() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("ok"));

    let tool = ToolDefinition::new(
        "echo",
        "Echo the input text",
        json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
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
    // Anthropic's shape is flat — name/description/input_schema at
    // the top level. NO outer `type: function` wrapper.
    assert_eq!(tools[0]["name"], "echo");
    assert_eq!(tools[0]["description"], "Echo the input text");
    assert_eq!(tools[0]["input_schema"]["type"], "object");
    assert!(tools[0].get("function").is_none(), "Anthropic tools must not be function-wrapped");
}

#[test]
fn tools_omitted_when_empty() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("hi"));

    provider
        .stream(ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        })
        .unwrap();

    let body: Value = serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert!(body.get("tools").is_none());
}

#[test]
fn malformed_tool_use_input_is_serialise_error_no_silent_dropping() {
    let (mut provider, _stub) = provider_with_stub();

    let assistant_message = ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![ContentBlock::ToolUse {
            id: "toolu_bad".into(),
            name: "echo".into(),
            input: "this is not json".into(),
        }],
    };

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![assistant_message],
        tools: vec![],
    });

    let err = result.unwrap_err();
    assert!(
        err.message().contains("not valid JSON"),
        "expected JSON parse error, got: {}",
        err.message()
    );
}

// ---------- response parsing ----------

#[test]
fn text_only_response_parses_into_text_delta_then_stop() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(200, ok_text_response("hello world"));

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
fn tool_use_response_emits_tool_use_with_input_serialised_to_string() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(
        200,
        ok_tool_use_response("toolu_99", "echo", json!({ "text": "hi" })),
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
            assert_eq!(id, "toolu_99");
            assert_eq!(name, "echo");
            // Input round-tripped to compact JSON string.
            assert_eq!(input, r#"{"text":"hi"}"#);
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
    matches!(events[1], AssistantEvent::MessageStop);
}

#[test]
fn mixed_text_and_tool_use_blocks_emit_in_order() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(
        200,
        json!({
            "id": "msg_y",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "text", "text": "checking" },
                { "type": "tool_use", "id": "toolu_z", "name": "x", "input": {} }
            ],
            "stop_reason": "tool_use"
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
fn thinking_blocks_silently_dropped_v32a() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(
        200,
        json!({
            "id": "msg_t",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "internal reasoning" },
                { "type": "text", "text": "answer" }
            ],
            "stop_reason": "end_turn"
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

    // Thinking dropped; only text + stop emitted.
    assert_eq!(events.len(), 2);
    match &events[0] {
        AssistantEvent::TextDelta(text) => assert_eq!(text, "answer"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

#[test]
fn response_with_only_thinking_is_runtime_error() {
    // If the model only emits thinking with no surfaceable content,
    // we surface an error rather than silently producing an empty
    // assistant turn (which would confuse the conversation loop).
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(
        200,
        json!({
            "id": "msg_t",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "..." }
            ],
            "stop_reason": "end_turn"
        })
        .to_string(),
    );

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert!(result.is_err());
}

// ---------- HTTP error handling — NO retry ----------

#[test]
fn http_500_is_runtime_error_no_retry() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(500, r#"{"type":"error","error":{"type":"api_error"}}"#);

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    let err = result.unwrap_err();
    assert!(err.message().contains("HTTP 500"));
    assert_eq!(stub.recorded_requests().len(), 1);
}

#[test]
fn http_429_is_runtime_error_no_retry() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(429, r#"{"error":"rate_limited"}"#);

    let _ = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert_eq!(stub.recorded_requests().len(), 1);
}

#[test]
fn http_401_is_runtime_error_no_fallback_to_anonymous() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_ok(401, r#"{"error":{"type":"authentication_error"}}"#);

    let _ = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    assert_eq!(stub.recorded_requests().len(), 1);
}

#[test]
fn transport_error_surfaces_as_runtime_error() {
    let (mut provider, stub) = provider_with_stub();
    stub.push_err("connection refused");

    let result = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });
    let err = result.unwrap_err();
    assert!(err.message().contains("connection refused"));
    assert_eq!(stub.recorded_requests().len(), 1);
}
