//! Anthropic Messages API provider.
//!
//! Different enough from OpenAI Chat Completions to need its own
//! impl — content blocks live inside `messages[].content` (no
//! separate `tool_calls`), system prompt is a top-level field, tool
//! results are sent as `user`-role messages, and auth uses
//! `x-api-key` (not `Authorization: Bearer`).
//!
//! Reference: <https://docs.anthropic.com/en/api/messages>
//!
//! V3.2a limitations (re-enter later):
//!   - Non-streaming only.
//!   - `x-api-key` auth only — `ANTHROPIC_AUTH_TOKEN` (Bearer for
//!     proxy / OAuth) deferred.
//!   - Extended `thinking` content blocks are silently dropped
//!     (the agent doesn't surface model internal reasoning yet).
//!   - No prompt-cache opt-in.

use crate::api::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolDefinition};
use crate::message::{ContentBlock, ConversationMessage, MessageRole};
use crate::providers::http::{friendly_http_status, HttpClient};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

const ANTHROPIC_VERSION: &str = "2023-06-01";

// ---------- public config + provider ----------

#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    /// Endpoint root. Default `https://api.anthropic.com`. Use a
    /// proxy URL here when routing through a corporate gateway.
    pub base_url: String,
    /// `sk-ant-...` API key. The Bearer / OAuth path is V3.2 follow-up.
    pub api_key: String,
    /// Model identifier (e.g. `claude-sonnet-4-5`, `claude-opus-4-7`,
    /// `claude-haiku-4-5`).
    pub model: String,
    /// Upper bound on tokens per turn.
    pub max_tokens: u32,
    /// `anthropic-version` header value. Defaults to `"2023-06-01"`.
    pub anthropic_version: String,
}

impl AnthropicConfig {
    /// Convenience: build from env. Reads `AEGIS_ANTHROPIC_API_KEY`
    /// (or `ANTHROPIC_API_KEY` as fallback) + `AEGIS_ANTHROPIC_MODEL`
    /// (no fallback — pick a model explicitly). Base URL defaults
    /// to `https://api.anthropic.com`; override via
    /// `AEGIS_ANTHROPIC_BASE_URL`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("AEGIS_ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .ok()
            .filter(|s| !s.is_empty())?;
        let model = std::env::var("AEGIS_ANTHROPIC_MODEL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let base_url = std::env::var("AEGIS_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".into());
        Some(Self {
            base_url,
            api_key,
            model,
            max_tokens: 4096,
            anthropic_version: ANTHROPIC_VERSION.into(),
        })
    }
}

pub struct AnthropicProvider {
    config: AnthropicConfig,
    http: Box<dyn HttpClient>,
}

impl AnthropicProvider {
    #[must_use]
    pub fn new(config: AnthropicConfig, http: Box<dyn HttpClient>) -> Self {
        Self { config, http }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        vec![
            ("content-type".into(), "application/json".into()),
            ("x-api-key".into(), self.config.api_key.clone()),
            (
                "anthropic-version".into(),
                self.config.anthropic_version.clone(),
            ),
        ]
    }
}

impl ApiClient for AnthropicProvider {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let messages_request = build_messages_request(&self.config, &request)?;
        let body = serde_json::to_string(&messages_request)
            .map_err(|e| RuntimeError::new(format!("serialise request failed: {e}")))?;

        let endpoint = self.endpoint();
        let headers = self.auth_headers();

        // Per V3 framing — NO retry. Failure goes straight back as
        // RuntimeError; agent surfaces as StoppedReason::ProviderError.
        let response = self
            .http
            .post(&endpoint, &headers, &body)
            .map_err(|e| RuntimeError::new(format!("HTTP transport error: {e}")))?;

        if response.status >= 400 {
            return Err(RuntimeError::new(friendly_http_status(
                &endpoint,
                response.status,
                &response.body,
            )));
        }

        let messages_response: MessagesResponse = serde_json::from_str(&response.body)
            .map_err(|e| RuntimeError::new(format!("parse response failed: {e}")))?;

        parse_response_to_events(messages_response)
    }
}

// ---------- internal wire types (Anthropic Messages API) ----------

#[derive(Debug, Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireTool>>,
}

#[derive(Debug, Serialize)]
struct WireMessage {
    role: String,
    content: Vec<WireContentBlockOut>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireContentBlockOut {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: JsonValue,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<WireToolResultBlock>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireToolResultBlock {
    Text { text: String },
}

#[derive(Debug, Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: JsonValue,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<WireContentBlockIn>,
    #[serde(default)]
    _stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireContentBlockIn {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: JsonValue,
    },
    /// Extended thinking — silently ignored in V3.2a.
    Thinking {
        #[serde(default)]
        _thinking: Option<String>,
    },
    /// Redacted thinking — silently ignored in V3.2a.
    RedactedThinking {
        #[serde(default)]
        _data: Option<JsonValue>,
    },
}

// ---------- mapping: aegis-agent → Anthropic request ----------

fn build_messages_request(
    config: &AnthropicConfig,
    request: &ApiRequest,
) -> Result<MessagesRequest, RuntimeError> {
    // Anthropic puts the system prompt in a top-level field, not a
    // message. Multiple system lines join into one block.
    let system = if request.system_prompt.is_empty() {
        None
    } else {
        Some(request.system_prompt.join("\n\n"))
    };

    let mut messages = Vec::new();
    for message in &request.messages {
        if let Some(wire) = map_conversation_message(message)? {
            messages.push(wire);
        }
    }

    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(request.tools.iter().map(map_tool).collect())
    };

    Ok(MessagesRequest {
        model: config.model.clone(),
        max_tokens: config.max_tokens,
        system,
        messages,
        tools,
    })
}

fn map_conversation_message(
    message: &ConversationMessage,
) -> Result<Option<WireMessage>, RuntimeError> {
    match message.role {
        MessageRole::System => {
            // System messages embedded in the conversation are
            // collapsed into the top-level `system` field by the
            // caller. We could re-emit them as user-role text, but
            // it's safer to drop them here so they don't confuse
            // downstream conversation accounting.
            Ok(None)
        }
        MessageRole::User => {
            let blocks: Vec<WireContentBlockOut> = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(WireContentBlockOut::Text {
                        text: text.clone(),
                    }),
                    _ => None,
                })
                .collect();
            if blocks.is_empty() {
                return Ok(None);
            }
            Ok(Some(WireMessage {
                role: "user".into(),
                content: blocks,
            }))
        }
        MessageRole::Assistant => {
            let mut blocks = Vec::new();
            for block in &message.blocks {
                match block {
                    ContentBlock::Text { text } => {
                        blocks.push(WireContentBlockOut::Text { text: text.clone() });
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        // Our internal model carries tool input as a
                        // JSON-encoded string; Anthropic expects a JSON
                        // object. Round-trip parse it.
                        let input_value: JsonValue = if input.is_empty() {
                            JsonValue::Object(serde_json::Map::new())
                        } else {
                            serde_json::from_str(input).map_err(|e| {
                                RuntimeError::new(format!(
                                    "tool_use input is not valid JSON: {e} — input was: {input}"
                                ))
                            })?
                        };
                        blocks.push(WireContentBlockOut::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input_value,
                        });
                    }
                    ContentBlock::ToolResult { .. } => {
                        // tool_result blocks belong on user-role
                        // messages in Anthropic; should never appear
                        // on assistant-role. Drop defensively.
                    }
                }
            }
            if blocks.is_empty() {
                return Ok(None);
            }
            Ok(Some(WireMessage {
                role: "assistant".into(),
                content: blocks,
            }))
        }
        MessageRole::Tool => {
            // In Anthropic's model, tool results are user-role
            // messages with tool_result content blocks.
            let blocks: Vec<WireContentBlockOut> = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => Some(WireContentBlockOut::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![WireToolResultBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    }),
                    _ => None,
                })
                .collect();
            if blocks.is_empty() {
                return Ok(None);
            }
            Ok(Some(WireMessage {
                role: "user".into(),
                content: blocks,
            }))
        }
    }
}

fn map_tool(tool: &ToolDefinition) -> WireTool {
    WireTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
    }
}

// ---------- mapping: Anthropic response → AssistantEvent ----------

fn parse_response_to_events(
    response: MessagesResponse,
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut text_buf = String::new();

    for block in response.content {
        match block {
            WireContentBlockIn::Text { text } => {
                text_buf.push_str(&text);
            }
            WireContentBlockIn::ToolUse { id, name, input } => {
                if !text_buf.is_empty() {
                    events.push(AssistantEvent::TextDelta(std::mem::take(&mut text_buf)));
                }
                let input_string = serde_json::to_string(&input).map_err(|e| {
                    RuntimeError::new(format!("serialise tool_use input failed: {e}"))
                })?;
                events.push(AssistantEvent::ToolUse {
                    id,
                    name,
                    input: input_string,
                });
            }
            WireContentBlockIn::Thinking { .. } | WireContentBlockIn::RedactedThinking { .. } => {
                // Silently drop — V3.2a doesn't surface model internals.
            }
        }
    }

    if !text_buf.is_empty() {
        events.push(AssistantEvent::TextDelta(text_buf));
    }

    if events.is_empty() {
        return Err(RuntimeError::new(
            "Anthropic response had no surfaceable content (only thinking blocks?)",
        ));
    }

    events.push(AssistantEvent::MessageStop);
    Ok(events)
}
