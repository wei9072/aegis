//! Streaming API contract for the conversation loop.
//!
//! Adapted from claw-code (MIT) — `rust/crates/runtime/src/conversation.rs`.
//! Token-usage and prompt-cache events trimmed; they re-enter in V3.7.

use crate::message::ConversationMessage;
use serde_json::Value as JsonValue;
use std::fmt::{Display, Formatter};

/// Definition of a tool the model can call. Mirrors the shape both
/// Anthropic Messages API and OpenAI Chat Completions accept (with
/// minor per-provider serialisation differences handled in
/// `providers/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's input. Stored as
    /// `serde_json::Value` so providers can re-shape it without
    /// parsing strings.
    pub input_schema: JsonValue,
}

impl ToolDefinition {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: JsonValue,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// Fully assembled request payload sent to the upstream model client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub system_prompt: Vec<String>,
    pub messages: Vec<ConversationMessage>,
    /// Tools the model is allowed to call this turn. Empty vec means
    /// "no tools available" — the model can still produce text but
    /// cannot emit `tool_use` blocks.
    pub tools: Vec<ToolDefinition>,
}

/// Streamed events emitted while processing a single assistant turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantEvent {
    TextDelta(String),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    MessageStop,
}

/// Minimal streaming API contract required by the conversation loop.
/// Implementors talk to a real LLM backend (Anthropic, OpenAI-compat,
/// etc.) or to a stub for tests.
pub trait ApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError>;
}

/// Blanket impl so callers can pass `Box<dyn ApiClient>` directly
/// to `ConversationRuntime::new` — useful for runtime provider
/// selection (V3.7 chat_demo, the `aegis chat` CLI, etc.).
impl<T: ApiClient + ?Sized> ApiClient for Box<T> {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        (**self).stream(request)
    }
}

/// Error returned when a conversation turn cannot be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    message: String,
}

impl RuntimeError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}
