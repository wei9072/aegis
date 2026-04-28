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

/// Capability mixin: providers that support runtime model switching.
/// REPL `/model <name>` calls this; non-supporting providers (e.g.
/// future fixed-model gateways) just don't impl it.
pub trait ConfigurableModel {
    fn set_model(&mut self, model: String);
    fn current_model(&self) -> &str;
}

/// Trait alias combining `ApiClient` + `ConfigurableModel`. The CLI
/// stores the picked provider as `Box<dyn ChatProvider>` so the REPL
/// can both stream messages and switch models without naming the
/// concrete type. Auto-impl covers anything that already implements
/// both traits (the three production providers all do).
pub trait ChatProvider: ApiClient + ConfigurableModel {}
impl<T: ApiClient + ConfigurableModel + ?Sized> ChatProvider for T {}

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
///
/// Two methods:
///   - `stream` — non-streaming default. Returns the full
///     `Vec<AssistantEvent>` once the model has finished. Backwards-
///     compatible with V3.1–V3.5; existing impls only need this.
///   - `stream_with_callback` — incremental delivery. Default impl
///     calls `stream` then replays the events through `on_event` so
///     non-streaming providers still drive the UI sensibly.
///     Providers that genuinely stream (V3.8 OpenAI-compat SSE)
///     override this method to invoke `on_event` per arriving chunk.
pub trait ApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError>;

    /// Optional incremental delivery. Default: call `stream`, then
    /// replay events through the callback in order. Override when
    /// the underlying transport actually streams.
    ///
    /// `on_event` is called once per `AssistantEvent` (including the
    /// terminal `MessageStop`). If the callback panics it propagates
    /// — providers do NOT swallow it.
    fn stream_with_callback(
        &mut self,
        request: ApiRequest,
        on_event: &mut dyn FnMut(&AssistantEvent),
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let events = self.stream(request)?;
        for event in &events {
            on_event(event);
        }
        Ok(events)
    }
}

/// Blanket impl so callers can pass `Box<dyn ApiClient>` directly
/// to `ConversationRuntime::new` — useful for runtime provider
/// selection (V3.7 chat_demo, the `aegis chat` CLI, etc.).
impl<T: ApiClient + ?Sized> ApiClient for Box<T> {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        (**self).stream(request)
    }

    fn stream_with_callback(
        &mut self,
        request: ApiRequest,
        on_event: &mut dyn FnMut(&AssistantEvent),
    ) -> Result<Vec<AssistantEvent>, RuntimeError> {
        (**self).stream_with_callback(request, on_event)
    }
}

/// Same trick for `ConfigurableModel`: forward through a Box. Lets
/// the CLI keep `Box<dyn ChatProvider>` as the runtime's `C` and
/// still call `set_model` from REPL.
impl<T: ConfigurableModel + ?Sized> ConfigurableModel for Box<T> {
    fn set_model(&mut self, model: String) {
        (**self).set_model(model);
    }
    fn current_model(&self) -> &str {
        (**self).current_model()
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
