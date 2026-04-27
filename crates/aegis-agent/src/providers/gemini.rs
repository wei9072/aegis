//! Google Gemini `generateContent` provider.
//!
//! Third wire format — different again from OpenAI Chat Completions
//! and Anthropic Messages:
//!   - URL-embedded model: `/v1beta/models/{model}:generateContent`
//!   - "model" role instead of "assistant"
//!   - `parts[]` instead of `content[]` blocks
//!   - `functionCall` / `functionResponse` parts (not tool_use blocks)
//!   - `systemInstruction` separate top-level field
//!   - `tools[].functionDeclarations[]` (one extra wrapper level)
//!   - Auth via `x-goog-api-key` header
//!
//! V3.2c limitations:
//!   - Non-streaming only.
//!   - No safety settings configuration (uses Google defaults).
//!   - No "thinking" mode for newer Gemini reasoning models.

use crate::api::{ApiClient, ApiRequest, AssistantEvent, RuntimeError};
use crate::message::{ContentBlock, ConversationMessage, MessageRole};
use crate::providers::http::{friendly_http_status, HttpClient};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Clone, Debug)]
pub struct GeminiConfig {
    /// Endpoint root. Default `https://generativelanguage.googleapis.com`.
    pub base_url: String,
    /// `AIza...` API key.
    pub api_key: String,
    /// Model name: `gemini-2.5-pro`, `gemini-2.5-flash`, etc.
    pub model: String,
    /// Max output tokens per turn.
    pub max_tokens: u32,
}

impl GeminiConfig {
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("AEGIS_GEMINI_API_KEY")
            .or_else(|_| std::env::var("GEMINI_API_KEY"))
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .ok()
            .filter(|s| !s.is_empty())?;
        let model = std::env::var("AEGIS_GEMINI_MODEL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let base_url = std::env::var("AEGIS_GEMINI_BASE_URL")
            .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".into());
        Some(Self {
            base_url,
            api_key,
            model,
            max_tokens: 4096,
        })
    }
}

pub struct GeminiProvider {
    config: GeminiConfig,
    http: Box<dyn HttpClient>,
}

impl GeminiProvider {
    #[must_use]
    pub fn new(config: GeminiConfig, http: Box<dyn HttpClient>) -> Self {
        Self { config, http }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent",
            self.config.base_url.trim_end_matches('/'),
            self.config.model
        )
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        vec![
            ("content-type".into(), "application/json".into()),
            ("x-goog-api-key".into(), self.config.api_key.clone()),
        ]
    }
}

impl ApiClient for GeminiProvider {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let gemini_request = build_gemini_request(&self.config, &request)?;
        let body = serde_json::to_string(&gemini_request)
            .map_err(|e| RuntimeError::new(format!("serialise request failed: {e}")))?;

        let endpoint = self.endpoint();
        let headers = self.auth_headers();

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

        let gemini_response: GenerateContentResponse = serde_json::from_str(&response.body)
            .map_err(|e| RuntimeError::new(format!("parse response failed: {e}")))?;

        parse_response_to_events(gemini_response)
    }
}

// ---------- internal wire types ----------

#[derive(Debug, Serialize)]
struct GenerateContentRequest {
    contents: Vec<WireContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<WireContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireToolGroup>>,
    #[serde(rename = "generationConfig")]
    generation_config: WireGenerationConfig,
}

#[derive(Debug, Serialize)]
struct WireContent {
    role: String,
    parts: Vec<WirePart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum WirePart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: WireFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: WireFunctionResponse,
    },
}

#[derive(Debug, Serialize)]
struct WireFunctionCall {
    name: String,
    args: JsonValue,
}

#[derive(Debug, Serialize)]
struct WireFunctionResponse {
    name: String,
    response: JsonValue,
}

#[derive(Debug, Serialize)]
struct WireToolGroup {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<WireFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct WireFunctionDeclaration {
    name: String,
    description: String,
    parameters: JsonValue,
}

#[derive(Debug, Serialize)]
struct WireGenerationConfig {
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<WireCandidate>,
}

#[derive(Debug, Deserialize)]
struct WireCandidate {
    #[serde(default)]
    content: Option<WireResponseContent>,
}

#[derive(Debug, Deserialize)]
struct WireResponseContent {
    #[serde(default)]
    parts: Vec<WireResponsePart>,
}

#[derive(Debug, Deserialize)]
struct WireResponsePart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "functionCall")]
    function_call: Option<WireResponseFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct WireResponseFunctionCall {
    name: String,
    #[serde(default)]
    args: JsonValue,
}

// ---------- mapping ----------

fn build_gemini_request(
    config: &GeminiConfig,
    request: &ApiRequest,
) -> Result<GenerateContentRequest, RuntimeError> {
    let system_instruction = if request.system_prompt.is_empty() {
        None
    } else {
        Some(WireContent {
            role: "user".into(),
            parts: vec![WirePart::Text {
                text: request.system_prompt.join("\n\n"),
            }],
        })
    };

    let mut contents = Vec::new();
    for message in &request.messages {
        if let Some(wire) = map_message(message)? {
            contents.push(wire);
        }
    }

    let tools = if request.tools.is_empty() {
        None
    } else {
        let declarations = request
            .tools
            .iter()
            .map(|tool| WireFunctionDeclaration {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            })
            .collect();
        Some(vec![WireToolGroup {
            function_declarations: declarations,
        }])
    };

    Ok(GenerateContentRequest {
        contents,
        system_instruction,
        tools,
        generation_config: WireGenerationConfig {
            max_output_tokens: config.max_tokens,
        },
    })
}

fn map_message(message: &ConversationMessage) -> Result<Option<WireContent>, RuntimeError> {
    match message.role {
        MessageRole::System => Ok(None), // collapsed into systemInstruction
        MessageRole::User => {
            let parts: Vec<WirePart> = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(WirePart::Text { text: text.clone() }),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                return Ok(None);
            }
            Ok(Some(WireContent {
                role: "user".into(),
                parts,
            }))
        }
        MessageRole::Assistant => {
            let mut parts = Vec::new();
            for block in &message.blocks {
                match block {
                    ContentBlock::Text { text } => parts.push(WirePart::Text { text: text.clone() }),
                    ContentBlock::ToolUse { name, input, .. } => {
                        let args: JsonValue = if input.is_empty() {
                            JsonValue::Object(serde_json::Map::new())
                        } else {
                            serde_json::from_str(input).map_err(|e| {
                                RuntimeError::new(format!(
                                    "tool_use input is not valid JSON: {e} — input was: {input}"
                                ))
                            })?
                        };
                        parts.push(WirePart::FunctionCall {
                            function_call: WireFunctionCall {
                                name: name.clone(),
                                args,
                            },
                        });
                    }
                    ContentBlock::ToolResult { .. } => {} // wrong role, drop
                }
            }
            if parts.is_empty() {
                return Ok(None);
            }
            Ok(Some(WireContent {
                role: "model".into(),
                parts,
            }))
        }
        MessageRole::Tool => {
            // Tool results are user-role messages with functionResponse parts.
            let parts: Vec<WirePart> = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolResult {
                        tool_name,
                        output,
                        is_error,
                        ..
                    } => {
                        let mut response = serde_json::Map::new();
                        response.insert("output".into(), JsonValue::String(output.clone()));
                        if *is_error {
                            response.insert("isError".into(), JsonValue::Bool(true));
                        }
                        Some(WirePart::FunctionResponse {
                            function_response: WireFunctionResponse {
                                name: tool_name.clone(),
                                response: JsonValue::Object(response),
                            },
                        })
                    }
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                return Ok(None);
            }
            Ok(Some(WireContent {
                role: "user".into(),
                parts,
            }))
        }
    }
}

fn parse_response_to_events(
    response: GenerateContentResponse,
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let candidate = response
        .candidates
        .into_iter()
        .next()
        .ok_or_else(|| RuntimeError::new("Gemini response had zero candidates"))?;
    let content = candidate
        .content
        .ok_or_else(|| RuntimeError::new("Gemini candidate had no content"))?;

    let mut events = Vec::new();
    let mut text_buf = String::new();
    let mut next_call_id: u64 = 0;

    for part in content.parts {
        if let Some(text) = part.text {
            text_buf.push_str(&text);
            continue;
        }
        if let Some(call) = part.function_call {
            if !text_buf.is_empty() {
                events.push(AssistantEvent::TextDelta(std::mem::take(&mut text_buf)));
            }
            // Gemini doesn't return a call id; synthesize one.
            let id = format!("gem_call_{next_call_id}");
            next_call_id += 1;
            let input_string = serde_json::to_string(&call.args).map_err(|e| {
                RuntimeError::new(format!("serialise functionCall args failed: {e}"))
            })?;
            events.push(AssistantEvent::ToolUse {
                id,
                name: call.name,
                input: input_string,
            });
        }
    }
    if !text_buf.is_empty() {
        events.push(AssistantEvent::TextDelta(text_buf));
    }
    if events.is_empty() {
        return Err(RuntimeError::new(
            "Gemini response had no surfaceable content",
        ));
    }
    events.push(AssistantEvent::MessageStop);
    Ok(events)
}
