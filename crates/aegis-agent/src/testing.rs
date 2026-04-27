//! Test scaffolding — stub `ApiClient` and `ToolExecutor` impls.
//!
//! Exposed so external test files (in `tests/`) can drive end-to-end
//! conversation scenarios without a live LLM. Both stubs are
//! script-driven: you push the events the LLM should emit and the
//! tool outputs to return, then `run_turn` plays them back.

use crate::api::{ApiClient, ApiRequest, AssistantEvent, RuntimeError};
use crate::tool::{ToolError, ToolExecutor};
use std::collections::VecDeque;

/// `ApiClient` that replays a queue of pre-recorded event lists.
/// Each `stream` call pops one list off the queue. If the queue is
/// empty, returns the configured "exhausted" error.
pub struct ScriptedApiClient {
    pub responses: VecDeque<Result<Vec<AssistantEvent>, RuntimeError>>,
    pub exhausted_error: String,
    pub recorded_requests: Vec<ApiRequest>,
}

impl ScriptedApiClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            responses: VecDeque::new(),
            exhausted_error: "scripted api client exhausted".to_string(),
            recorded_requests: Vec::new(),
        }
    }

    pub fn push_text_then_done(mut self, text: impl Into<String>) -> Self {
        self.responses.push_back(Ok(vec![
            AssistantEvent::TextDelta(text.into()),
            AssistantEvent::MessageStop,
        ]));
        self
    }

    pub fn push_tool_call(
        mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        self.responses.push_back(Ok(vec![
            AssistantEvent::ToolUse {
                id: id.into(),
                name: name.into(),
                input: input.into(),
            },
            AssistantEvent::MessageStop,
        ]));
        self
    }

    pub fn push_error(mut self, message: impl Into<String>) -> Self {
        self.responses.push_back(Err(RuntimeError::new(message.into())));
        self
    }
}

impl Default for ScriptedApiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiClient for ScriptedApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.recorded_requests.push(request);
        self.responses
            .pop_front()
            .unwrap_or_else(|| Err(RuntimeError::new(self.exhausted_error.clone())))
    }
}

/// `ToolExecutor` that maps tool name → fixed output. A name not in
/// the map yields `ToolError("no such tool: <name>")`.
pub struct ScriptedToolExecutor {
    pub handlers: std::collections::BTreeMap<String, Result<String, ToolError>>,
    pub recorded_calls: Vec<(String, String)>,
}

impl ScriptedToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Default::default(),
            recorded_calls: Vec::new(),
        }
    }

    pub fn with_ok(mut self, name: impl Into<String>, output: impl Into<String>) -> Self {
        self.handlers.insert(name.into(), Ok(output.into()));
        self
    }

    pub fn with_err(mut self, name: impl Into<String>, error: impl Into<String>) -> Self {
        self.handlers
            .insert(name.into(), Err(ToolError::new(error.into())));
        self
    }
}

impl Default for ScriptedToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor for ScriptedToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        self.recorded_calls
            .push((tool_name.to_string(), input.to_string()));
        match self.handlers.get(tool_name) {
            Some(Ok(output)) => Ok(output.clone()),
            Some(Err(error)) => Err(ToolError::new(error.message().to_string())),
            None => Err(ToolError::new(format!("no such tool: {tool_name}"))),
        }
    }
}
