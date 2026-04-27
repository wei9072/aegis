//! Conversation runtime — the multi-turn loop body.
//!
//! Adapted from claw-code (MIT) —
//! `rust/crates/runtime/src/conversation.rs::ConversationRuntime`.
//!
//! V3.1 simplifications relative to upstream:
//!   - No hooks (PreToolUse / PostToolUse) — V3.6 lands those.
//!   - No permission policy — V3.6 lands those.
//!   - No auto-compaction — V3.7.
//!   - No telemetry / session tracer — out of scope.
//!   - No prompt-cache events.
//!   - No token-usage tracking — V3.7.
//!   - Returns `AgentTurnResult` (aegis-agent's framing-aware shape)
//!     instead of `TurnSummary` — verifier integration points (V3.4)
//!     plug in via the `task_verdict` field.
//!
//! The aegis-specific differentiation points (PreToolUse aegis-predict,
//! cross-turn cost tracking, verifier-driven done, stalemate detection)
//! land in V3.2–V3.5; this phase is the loop skeleton only.

use crate::api::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolDefinition};
use crate::cost::CostTracker;
use crate::message::{ContentBlock, ConversationMessage, Session};
use crate::predict::{NullPredictor, PreToolUsePredictor, PredictVerdict};
use crate::tool::ToolExecutor;
use crate::{AgentConfig, AgentTurnResult, StoppedReason};

use serde_json::Value;

/// Optional callback invoked after every tool execution to give the
/// runtime a chance to observe per-file cost. Receives the tool name
/// + raw input string and returns `(path, cost)` pairs to record
/// into the cost tracker. The default impl returns `None` (skip
/// cost tracking entirely).
pub trait CostObserver: Send {
    fn observe(&mut self, tool_name: &str, input: &str) -> Vec<(std::path::PathBuf, f64)>;
}

/// No-op cost observer — cost tracking off until the user wires
/// something real (the V3.3+ aegis observer ports `aegis-core`'s
/// signal extraction here).
pub struct NullCostObserver;
impl CostObserver for NullCostObserver {
    fn observe(
        &mut self,
        _tool_name: &str,
        _input: &str,
    ) -> Vec<(std::path::PathBuf, f64)> {
        Vec::new()
    }
}

/// Coordinates the model loop and tool execution.
pub struct ConversationRuntime<C, T> {
    session: Session,
    api_client: C,
    tool_executor: T,
    system_prompt: Vec<String>,
    tools: Vec<ToolDefinition>,
    config: AgentConfig,
    predictor: Box<dyn PreToolUsePredictor>,
    cost_observer: Box<dyn CostObserver>,
    cost_tracker: CostTracker,
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    #[must_use]
    pub fn new(
        session: Session,
        api_client: C,
        tool_executor: T,
        system_prompt: Vec<String>,
        tools: Vec<ToolDefinition>,
        config: AgentConfig,
    ) -> Self {
        Self {
            session,
            api_client,
            tool_executor,
            system_prompt,
            tools,
            config,
            predictor: Box::new(NullPredictor),
            cost_observer: Box::new(NullCostObserver),
            cost_tracker: CostTracker::new(),
        }
    }

    /// Inject a `PreToolUsePredictor`. Default is `NullPredictor`
    /// which allows everything.
    #[must_use]
    pub fn with_predictor(mut self, predictor: Box<dyn PreToolUsePredictor>) -> Self {
        self.predictor = predictor;
        self
    }

    /// Inject a `CostObserver` that runs after each tool execution
    /// to feed the per-session cost tracker. Default is
    /// `NullCostObserver` (cost tracking off).
    #[must_use]
    pub fn with_cost_observer(mut self, observer: Box<dyn CostObserver>) -> Self {
        self.cost_observer = observer;
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn into_session(self) -> Session {
        self.session
    }

    pub fn cost_tracker(&self) -> &CostTracker {
        &self.cost_tracker
    }

    /// Run one user turn through the model. Loops on tool_use until
    /// the assistant emits a turn with no `tool_use` blocks, or until
    /// the per-turn iteration budget is exhausted.
    ///
    /// Returns an `AgentTurnResult`. The result NEVER contains a
    /// retry signal — if the turn ends in `MaxIterations`, that is
    /// the agent reporting an observation; the caller decides whether
    /// to start another turn.
    pub fn run_turn(&mut self, user_input: impl Into<String>) -> AgentTurnResult {
        let user_input = user_input.into();
        self.session.push(ConversationMessage::user_text(user_input));

        let mut iterations: u32 = 0;
        let max_iterations = self.config.max_iterations_per_turn.max(1);

        loop {
            iterations = iterations.saturating_add(1);
            if iterations > max_iterations {
                return AgentTurnResult {
                    stopped_reason: StoppedReason::MaxIterations,
                    iterations: iterations - 1,
                    task_verdict: None,
                };
            }

            let request = ApiRequest {
                system_prompt: self.system_prompt.clone(),
                messages: self.session.messages.clone(),
                tools: self.tools.clone(),
            };

            let events = match self.api_client.stream(request) {
                Ok(events) => events,
                Err(error) => {
                    return AgentTurnResult {
                        stopped_reason: StoppedReason::ProviderError(error.message().to_string()),
                        iterations,
                        task_verdict: None,
                    };
                }
            };

            let assistant_message = match build_assistant_message(events) {
                Ok(message) => message,
                Err(error) => {
                    return AgentTurnResult {
                        stopped_reason: StoppedReason::ProviderError(error.message().to_string()),
                        iterations,
                        task_verdict: None,
                    };
                }
            };

            let pending_tool_uses: Vec<(String, String, String)> = assistant_message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            self.session.push(assistant_message);

            // No more tool calls — LLM signals "done". V3.4 will run
            // the verifier here and map to PlanDoneVerified /
            // PlanDoneVerifierRejected. For V3.1 we have no verifier,
            // so the only reachable variant is PlanDoneNoVerifier.
            if pending_tool_uses.is_empty() {
                return AgentTurnResult {
                    stopped_reason: StoppedReason::PlanDoneNoVerifier,
                    iterations,
                    task_verdict: None,
                };
            }

            // Execute each tool call. Failures flow back to the LLM
            // as `ToolResult { is_error: true }` — the LLM's own
            // agency decides what to do next iteration. The runtime
            // never coaches.
            for (tool_use_id, tool_name, input) in pending_tool_uses {
                // V3.3 differentiation #1: PreToolUse aegis-predict.
                // Ask the predictor whether this call should run.
                // BLOCK becomes a synthetic tool_result the LLM sees
                // (no actual tool execution, no fix suggestion).
                let predict_verdict = self.predictor.predict(&tool_name, &input);
                let (output, is_error) = match predict_verdict {
                    PredictVerdict::Block { reason } => (reason, true),
                    PredictVerdict::Allow => match self.tool_executor.execute(&tool_name, &input) {
                        Ok(output) => (output, false),
                        Err(error) => (error.message().to_string(), true),
                    },
                };

                // V3.3 differentiation #2: cross-turn cost tracking.
                // After (attempted) execution, ask the cost observer
                // for any per-file cost it can attribute to this
                // call. Observer returns empty list when it can't
                // attribute — that's fine.
                if !is_error {
                    let observations = self.cost_observer.observe(&tool_name, &input);
                    for (path, cost) in observations {
                        self.cost_tracker.observe(path, cost);
                    }
                }

                let result_message = ConversationMessage::tool_result(
                    tool_use_id,
                    tool_name,
                    output,
                    is_error,
                );
                self.session.push(result_message);
            }

            // V3.3: between iterations, check the session cost budget.
            // If exceeded, terminate immediately — no retry, no
            // coaching string back to the LLM. The user (or upstream
            // orchestrator) decides whether to start a fresh session.
            if let Some(budget) = self.config.session_cost_budget {
                let regression = self.cost_tracker.cumulative_regression();
                if regression > budget {
                    return AgentTurnResult {
                        stopped_reason: StoppedReason::CostBudgetExceeded,
                        iterations,
                        task_verdict: None,
                    };
                }
            }
        }
    }
}

#[allow(dead_code)]
/// Helper used by tests to peek at cost-observation parsing logic
/// (currently inlined; this stub keeps the import linkable if
/// future refactors externalise it).
fn _value_of_path(_v: &Value) -> Option<String> {
    None
}

/// Collapse a stream of events into one assistant message.
///
/// Adapted from claw-code (MIT) —
/// `rust/crates/runtime/src/conversation.rs::build_assistant_message`.
/// Token-usage and prompt-cache event collection trimmed.
fn build_assistant_message(
    events: Vec<AssistantEvent>,
) -> Result<ConversationMessage, RuntimeError> {
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut finished = false;

    for event in events {
        match event {
            AssistantEvent::TextDelta(delta) => text.push_str(&delta),
            AssistantEvent::ToolUse { id, name, input } => {
                flush_text_block(&mut text, &mut blocks);
                blocks.push(ContentBlock::ToolUse { id, name, input });
            }
            AssistantEvent::MessageStop => {
                finished = true;
            }
        }
    }

    flush_text_block(&mut text, &mut blocks);

    if !finished {
        return Err(RuntimeError::new(
            "assistant stream ended without a message stop event",
        ));
    }
    if blocks.is_empty() {
        return Err(RuntimeError::new("assistant stream produced no content"));
    }

    Ok(ConversationMessage::assistant(blocks))
}

fn flush_text_block(text: &mut String, blocks: &mut Vec<ContentBlock>) {
    if !text.is_empty() {
        blocks.push(ContentBlock::Text {
            text: std::mem::take(text),
        });
    }
}
