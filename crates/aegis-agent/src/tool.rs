//! Tool execution contract.
//!
//! Adapted from claw-code (MIT) — `rust/crates/runtime/src/conversation.rs`.

use std::fmt::{Display, Formatter};

/// Trait implemented by tool dispatchers that execute model-requested tools.
///
/// Tool errors flow back to the LLM as a `ToolResult` with `is_error =
/// true` (the LLM decides what to do — its own agency). The conversation
/// loop never converts the error into a hint string and prepends it to
/// the next prompt — see `tests/no_coaching_injection.rs` for the
/// structural fence.
pub trait ToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError>;
}

/// Blanket impl so callers can pass `Box<dyn ToolExecutor>` directly
/// (multi-source tool dispatchers, MCP-backed executors, etc.).
impl<T: ToolExecutor + ?Sized> ToolExecutor for Box<T> {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        (**self).execute(tool_name, input)
    }
}

/// Error returned when a tool invocation fails locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
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

impl Display for ToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}
