//! OpenAI Chat Completions provider.
//!
//! One impl, many backends — pick `base_url` to talk to:
//!
//! | Backend | base_url |
//! | --- | --- |
//! | OpenRouter | `https://openrouter.ai/api/v1` |
//! | Groq | `https://api.groq.com/openai/v1` |
//! | OpenAI (proper) | `https://api.openai.com/v1` |
//! | Ollama (local) | `http://127.0.0.1:11434/v1` |
//! | vLLM (self-hosted) | `http://your-vllm-host/v1` |
//! | llama.cpp server | `http://127.0.0.1:8080/v1` |
//! | LMStudio | `http://127.0.0.1:1234/v1` |
//! | DashScope (Qwen) | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
//!
//! Wire types are inlined here rather than in a separate file —
//! they're internal to the provider; nothing else in the crate
//! sees them. If a provider grows past ~600 LOC, split into
//! `wire.rs` then.
//!
//! V3.1b limitations (re-enter in later phases):
//!   - Non-streaming only (single POST → JSON response)
//!   - No reasoning-model parameters (reasoning_effort, etc.)
//!   - No prompt-cache opt-in
//!   - No tool_choice control (always "auto" — model decides)

use crate::api::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolDefinition};
use crate::message::{ContentBlock, ConversationMessage, MessageRole};
use crate::providers::http::HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ---------- public config + provider ----------

#[derive(Clone, Debug)]
pub struct OpenAiCompatConfig {
    /// Base URL like `https://api.openrouter.ai/api/v1`. The
    /// `/chat/completions` suffix is appended automatically — pass
    /// the version-prefixed root without trailing slash.
    pub base_url: String,
    /// Bearer token. `None` means no auth header is sent — use this
    /// for local Ollama / llama.cpp where auth is typically off.
    pub api_key: Option<String>,
    /// Model identifier as the backend expects it
    /// (e.g. `meta-llama/llama-3.3-70b-instruct` for OpenRouter,
    /// `llama3.2` for Ollama, `qwen-plus` for DashScope).
    pub model: String,
    /// Upper bound on tokens the model is allowed to emit per turn.
    pub max_tokens: u32,
}

impl OpenAiCompatConfig {
    /// Convenience: build a config from env vars. Reads
    /// `AEGIS_OPENAI_BASE_URL`, `AEGIS_OPENAI_API_KEY`,
    /// `AEGIS_OPENAI_MODEL`. Returns `None` if any required var is
    /// missing or empty (api_key is allowed to be missing).
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("AEGIS_OPENAI_BASE_URL").ok()?;
        let model = std::env::var("AEGIS_OPENAI_MODEL").ok()?;
        if base_url.is_empty() || model.is_empty() {
            return None;
        }
        let api_key = std::env::var("AEGIS_OPENAI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Some(Self {
            base_url,
            api_key,
            model,
            max_tokens: 4096,
        })
    }
}

pub struct OpenAiCompatProvider {
    config: OpenAiCompatConfig,
    http: Box<dyn HttpClient>,
}

impl OpenAiCompatProvider {
    #[must_use]
    pub fn new(config: OpenAiCompatConfig, http: Box<dyn HttpClient>) -> Self {
        Self { config, http }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![("content-type".into(), "application/json".into())];
        if let Some(key) = &self.config.api_key {
            headers.push(("authorization".into(), format!("Bearer {key}")));
        }
        headers
    }
}

impl ApiClient for OpenAiCompatProvider {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let chat_request = build_chat_request(&self.config, &request);
        let body = serde_json::to_string(&chat_request)
            .map_err(|e| RuntimeError::new(format!("serialise request failed: {e}")))?;

        let endpoint = self.endpoint();
        let headers = self.auth_headers();

        // Network IO. Per V3 framing — NO retry on transient errors.
        // Failure goes straight back as RuntimeError; the agent
        // surfaces it as StoppedReason::ProviderError; the caller
        // (user / orchestrator) decides whether to start a new turn.
        let response = self
            .http
            .post(&endpoint, &headers, &body)
            .map_err(|e| RuntimeError::new(format!("HTTP transport error: {e}")))?;

        if response.status >= 400 {
            return Err(RuntimeError::new(format!(
                "HTTP {} from {}: {}",
                response.status, endpoint, response.body
            )));
        }

        let chat_response: ChatResponse = serde_json::from_str(&response.body)
            .map_err(|e| RuntimeError::new(format!("parse response failed: {e}")))?;

        parse_response_to_events(chat_response)
    }
}

// ---------- internal wire types (OpenAI Chat Completions) ----------

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatTool>>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    #[serde(rename = "type")]
    kind: &'static str, // always "function" for OpenAI Chat Completions
    function: ChatToolFunction,
}

#[derive(Debug, Serialize)]
struct ChatToolFunction {
    name: String,
    description: String,
    parameters: JsonValue,
}

#[derive(Debug, Serialize)]
struct ChatToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: ChatToolCallFunction,
}

#[derive(Debug, Serialize)]
struct ChatToolCallFunction {
    name: String,
    /// JSON-encoded arguments string. We keep it as-is from the LLM
    /// so the tool executor sees exactly what the model emitted.
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatResponseChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatResponseToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseToolCall {
    id: String,
    #[serde(rename = "type", default)]
    _kind: Option<String>,
    function: ChatResponseToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct ChatResponseToolCallFunction {
    name: String,
    arguments: String,
}

// ---------- mapping: aegis-agent → OpenAI request ----------

fn build_chat_request(config: &OpenAiCompatConfig, request: &ApiRequest) -> ChatRequest {
    let mut messages = Vec::new();

    // System prompt collapses to a single OpenAI "system" message.
    if !request.system_prompt.is_empty() {
        messages.push(ChatMessage {
            role: "system".into(),
            content: Some(request.system_prompt.join("\n\n")),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
    }

    for message in &request.messages {
        messages.extend(map_conversation_message(message));
    }

    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(request.tools.iter().map(map_tool).collect())
    };

    ChatRequest {
        model: config.model.clone(),
        max_tokens: config.max_tokens,
        messages,
        tools,
    }
}

fn map_conversation_message(message: &ConversationMessage) -> Vec<ChatMessage> {
    match message.role {
        MessageRole::User => {
            // User messages are always plain text in V3.1.
            let text = collect_text(&message.blocks);
            vec![ChatMessage {
                role: "user".into(),
                content: Some(text),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }]
        }
        MessageRole::Assistant => {
            // Assistant message may contain text + tool_use. Emit as
            // a single OpenAI "assistant" message with both `content`
            // (text) and `tool_calls` (tool_use entries).
            let text = collect_text(&message.blocks);
            let tool_calls: Vec<ChatToolCall> = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => Some(ChatToolCall {
                        id: id.clone(),
                        kind: "function",
                        function: ChatToolCallFunction {
                            name: name.clone(),
                            arguments: input.clone(),
                        },
                    }),
                    _ => None,
                })
                .collect();
            vec![ChatMessage {
                role: "assistant".into(),
                content: if text.is_empty() { None } else { Some(text) },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
                name: None,
            }]
        }
        MessageRole::Tool => {
            // Each ToolResult block becomes its own OpenAI "tool"
            // message keyed by tool_call_id.
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        tool_name,
                        output,
                        is_error,
                    } => {
                        // OpenAI "tool" messages don't have a separate
                        // is_error field — convention is to prepend a
                        // marker so the model can see the error state.
                        let body = if *is_error {
                            format!("[error] {output}")
                        } else {
                            output.clone()
                        };
                        Some(ChatMessage {
                            role: "tool".into(),
                            content: Some(body),
                            tool_calls: None,
                            tool_call_id: Some(tool_use_id.clone()),
                            name: Some(tool_name.clone()),
                        })
                    }
                    _ => None,
                })
                .collect()
        }
        MessageRole::System => {
            // System messages can also live in the conversation if
            // the caller pushed them; pass through.
            vec![ChatMessage {
                role: "system".into(),
                content: Some(collect_text(&message.blocks)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }]
        }
    }
}

fn collect_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn map_tool(tool: &ToolDefinition) -> ChatTool {
    ChatTool {
        kind: "function",
        function: ChatToolFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

// ---------- mapping: OpenAI response → AssistantEvent ----------

fn parse_response_to_events(response: ChatResponse) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| RuntimeError::new("OpenAI response had zero choices"))?;

    let message = choice.message;
    let mut events = Vec::new();

    if let Some(text) = message.content {
        if !text.is_empty() {
            events.push(AssistantEvent::TextDelta(text));
        }
    }

    if let Some(tool_calls) = message.tool_calls {
        for call in tool_calls {
            events.push(AssistantEvent::ToolUse {
                id: call.id,
                name: call.function.name,
                input: call.function.arguments,
            });
        }
    }

    if events.is_empty() {
        return Err(RuntimeError::new(
            "OpenAI response had neither content nor tool_calls",
        ));
    }

    events.push(AssistantEvent::MessageStop);
    Ok(events)
}
