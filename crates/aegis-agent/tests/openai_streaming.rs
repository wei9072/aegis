//! V3.8 — OpenAI-compat streaming (`stream_with_callback`).
//!
//! Stub HTTP returns canned SSE frames; provider parses chunks and
//! invokes the callback per event.

use aegis_agent::api::{ApiClient, ApiRequest, AssistantEvent};
use aegis_agent::message::ConversationMessage;
use aegis_agent::providers::{
    HttpClient, OpenAiCompatConfig, OpenAiCompatProvider, StubHttpClient,
};
use std::sync::Arc;

struct StubRef(Arc<StubHttpClient>);
impl HttpClient for StubRef {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<aegis_agent::providers::HttpResponse, aegis_agent::providers::HttpError> {
        self.0.post(url, headers, body)
    }
}

fn make() -> (OpenAiCompatProvider, Arc<StubHttpClient>) {
    let stub = Arc::new(StubHttpClient::new());
    let provider = OpenAiCompatProvider::new(
        OpenAiCompatConfig {
            base_url: "https://example.test/v1".into(),
            api_key: Some("sk-x".into()),
            model: "test".into(),
            max_tokens: 100,
        },
        Box::new(StubRef(stub.clone())),
    );
    (provider, stub)
}

const SSE_TEXT_RESPONSE: &str = "\
data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\
\n\
data: [DONE]\n\
\n";

const SSE_TOOL_CALL_RESPONSE: &str = "\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"echo\"}}]}}]}\n\
\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"x\\\":1}\"}}]}}]}\n\
\n\
data: [DONE]\n\
\n";

#[test]
fn stream_with_callback_emits_text_chunks_in_order() {
    let (mut provider, stub) = make();
    stub.push_ok(200, SSE_TEXT_RESPONSE);

    let mut received: Vec<AssistantEvent> = Vec::new();
    let events = provider
        .stream_with_callback(
            ApiRequest {
                system_prompt: vec![],
                messages: vec![ConversationMessage::user_text("hi")],
                tools: vec![],
            },
            &mut |ev| received.push(ev.clone()),
        )
        .unwrap();

    // Three text deltas + MessageStop.
    assert_eq!(received.len(), 4);
    match &received[0] {
        AssistantEvent::TextDelta(text) => assert_eq!(text, "Hello"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
    match &received[1] {
        AssistantEvent::TextDelta(text) => assert_eq!(text, " world"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
    match &received[2] {
        AssistantEvent::TextDelta(text) => assert_eq!(text, "!"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
    matches!(received[3], AssistantEvent::MessageStop);

    // Returned vec mirrors callback order.
    assert_eq!(events.len(), received.len());
}

#[test]
fn stream_request_body_marks_stream_true() {
    let (mut provider, stub) = make();
    stub.push_ok(200, SSE_TEXT_RESPONSE);

    let _ = provider.stream_with_callback(
        ApiRequest {
            system_prompt: vec![],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        },
        &mut |_| {},
    );

    let body: serde_json::Value =
        serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert_eq!(body["stream"], true);
}

#[test]
fn non_streaming_request_omits_stream_field() {
    let (mut provider, stub) = make();
    stub.push_ok(
        200,
        r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#,
    );

    let _ = provider.stream(ApiRequest {
        system_prompt: vec![],
        messages: vec![ConversationMessage::user_text("hi")],
        tools: vec![],
    });

    let body: serde_json::Value =
        serde_json::from_str(&stub.recorded_requests()[0].body).unwrap();
    assert!(body.get("stream").is_none());
}

#[test]
fn streaming_tool_call_accumulates_across_chunks() {
    let (mut provider, stub) = make();
    stub.push_ok(200, SSE_TOOL_CALL_RESPONSE);

    let mut received: Vec<AssistantEvent> = Vec::new();
    let events = provider
        .stream_with_callback(
            ApiRequest {
                system_prompt: vec![],
                messages: vec![ConversationMessage::user_text("call echo")],
                tools: vec![],
            },
            &mut |ev| received.push(ev.clone()),
        )
        .unwrap();

    // ToolUse + MessageStop.
    assert_eq!(events.len(), 2);
    match &events[0] {
        AssistantEvent::ToolUse { id, name, input } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "echo");
            assert_eq!(input, r#"{"x":1}"#);
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
    matches!(events[1], AssistantEvent::MessageStop);
}

#[test]
fn streaming_skips_malformed_data_lines_no_panic() {
    let (mut provider, stub) = make();
    let body = "\
data: not valid json\n\
\n\
data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\
\n\
data: [DONE]\n\
\n";
    stub.push_ok(200, body);

    let mut received = 0;
    let _ = provider
        .stream_with_callback(
            ApiRequest {
                system_prompt: vec![],
                messages: vec![ConversationMessage::user_text("hi")],
                tools: vec![],
            },
            &mut |_| received += 1,
        )
        .unwrap();
    assert_eq!(received, 2); // 1 text + 1 stop
}
