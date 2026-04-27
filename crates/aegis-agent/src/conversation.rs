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
use crate::permission::{PermissionDecision, PermissionPolicy};
use crate::predict::{NullPredictor, PreToolUsePredictor, PredictVerdict};
use crate::stalemate::{StalemateDetector, StalemateVerdict};
use crate::tool::ToolExecutor;
use crate::verifier::AgentTaskVerifier;
use crate::{AgentConfig, AgentTurnResult, StoppedReason};

use aegis_decision::{TaskPattern, TaskVerdict};
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
    verifier: Option<Box<dyn AgentTaskVerifier>>,
    stalemate: StalemateDetector,
    permission_policy: Option<PermissionPolicy>,
    /// Optional per-event callback (V3.8 streaming). When set, the
    /// runtime invokes `stream_with_callback` and forwards each
    /// arriving `AssistantEvent` to this callback. Default = no-op
    /// (the runtime uses the non-streaming `stream` method).
    event_callback: Option<Box<dyn FnMut(&AssistantEvent) + Send>>,
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
            verifier: None,
            stalemate: StalemateDetector::new(),
            permission_policy: None,
            event_callback: None,
        }
    }

    /// Subscribe to per-event streaming. Useful for the chat REPL —
    /// each `AssistantEvent::TextDelta` arrives as the LLM streams
    /// it, so the user sees text appear instead of waiting for the
    /// full response.
    ///
    /// For providers that don't truly stream (Anthropic / Gemini in
    /// V3.8 — non-streaming impls), the default `ApiClient`
    /// implementation replays the full event vec through the callback
    /// once `stream` returns. UX degrades gracefully.
    #[must_use]
    pub fn with_event_callback(
        mut self,
        callback: Box<dyn FnMut(&AssistantEvent) + Send>,
    ) -> Self {
        self.event_callback = Some(callback);
        self
    }

    /// Non-consuming variant of `with_event_callback`. Lets the REPL
    /// install a fresh callback per turn (callback closures capture
    /// per-turn rendering state) without rebuilding the runtime.
    pub fn set_event_callback(
        &mut self,
        callback: Option<Box<dyn FnMut(&AssistantEvent) + Send>>,
    ) {
        self.event_callback = callback;
    }

    /// Apply a permission policy. Tool calls denied by the policy
    /// short-circuit before reaching the predictor or executor.
    /// Default = no policy (everything allowed at this layer; the
    /// predictor / executor still get a chance).
    #[must_use]
    pub fn with_permission_policy(mut self, policy: PermissionPolicy) -> Self {
        self.permission_policy = Some(policy);
        self
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

    /// Inject a task-level verifier (V3.4 differentiation #3).
    /// When the LLM signals "done" by emitting no further tool_use,
    /// the verifier runs against `config.workspace_root`. Its
    /// verdict overrides the LLM's claim:
    /// - `passed: true`  → `StoppedReason::PlanDoneVerified`
    /// - `passed: false` → `StoppedReason::PlanDoneVerifierRejected`
    ///
    /// No verifier configured → `StoppedReason::PlanDoneNoVerifier`
    /// (the LLM's word is taken at face value, but distinguished from
    /// the verifier-confirmed case so callers know which it was).
    #[must_use]
    pub fn with_verifier(mut self, verifier: Box<dyn AgentTaskVerifier>) -> Self {
        self.verifier = Some(verifier);
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

    /// Wipe the conversation transcript + cost tracker + stalemate
    /// detector and start fresh. Used by `/reset` in the chat REPL.
    /// The provider, executor, predictor, verifier, permissions and
    /// config stay — only the per-session state resets.
    pub fn reset_session(&mut self) {
        self.session = Session::new();
        self.cost_tracker = CostTracker::new();
        self.stalemate = StalemateDetector::new();
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

            let stream_result = if let Some(cb) = self.event_callback.as_mut() {
                self.api_client.stream_with_callback(request, cb.as_mut())
            } else {
                self.api_client.stream(request)
            };
            let events = match stream_result {
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

            // LLM signals "done" by emitting no further tool_use.
            // V3.4: if a verifier is wired in, its verdict is the
            // source of truth. Verdict goes to AgentTurnResult.task_verdict
            // for the user; it's NEVER converted into a coaching
            // string for the next prompt (no_coaching_injection.rs).
            if pending_tool_uses.is_empty() {
                let (stopped_reason, task_verdict) = match &self.verifier {
                    None => (StoppedReason::PlanDoneNoVerifier, None),
                    Some(verifier) => {
                        let workspace = self
                            .config
                            .workspace_root
                            .clone()
                            .unwrap_or_else(|| std::path::PathBuf::from("."));
                        let result = verifier.verify(&workspace);
                        let verdict = TaskVerdict {
                            pattern: if result.passed {
                                TaskPattern::Solved
                            } else {
                                TaskPattern::Incomplete
                            },
                            verifier_result: Some(result.clone()),
                            pipeline_done: true,
                            iterations_run: iterations,
                            error: String::new(),
                        };
                        let reason = if result.passed {
                            StoppedReason::PlanDoneVerified
                        } else {
                            StoppedReason::PlanDoneVerifierRejected
                        };
                        (reason, Some(verdict))
                    }
                };
                return AgentTurnResult {
                    stopped_reason,
                    iterations,
                    task_verdict,
                };
            }

            // Execute each tool call. Failures flow back to the LLM
            // as `ToolResult { is_error: true }` — the LLM's own
            // agency decides what to do next iteration. The runtime
            // never coaches.
            for (tool_use_id, tool_name, input) in pending_tool_uses {
                // V3.6: permission gate first — if the user's mode
                // doesn't allow this tool, deny before consulting
                // the predictor or executor (no point asking
                // aegis-mcp about a write that's already banned).
                let permission = match &self.permission_policy {
                    Some(policy) => policy.authorize(&tool_name),
                    None => PermissionDecision::Allow,
                };
                let (output, is_error) = match permission {
                    PermissionDecision::Deny { reason } => (reason, true),
                    PermissionDecision::Allow => {
                        // V3.3 differentiation #1: PreToolUse aegis-predict.
                        let predict_verdict = self.predictor.predict(&tool_name, &input);
                        match predict_verdict {
                            PredictVerdict::Block { reason } => (reason, true),
                            PredictVerdict::Allow => {
                                match self.tool_executor.execute(&tool_name, &input) {
                                    Ok(output) => (output, false),
                                    Err(error) => (error.message().to_string(), true),
                                }
                            }
                        }
                    }
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

            // V3.5: feed the cost-total into the stalemate detector.
            // Three successive iterations with the same total → the
            // LLM is going in circles; terminate with a named reason
            // rather than letting max_iterations swallow it silently.
            let snap = self.cost_tracker.snapshot();
            if !snap.is_empty() {
                let total: f64 = snap.iter().map(|e| e.current).sum();
                if let StalemateVerdict::StateStalemate = self.stalemate.record(total) {
                    return AgentTurnResult {
                        stopped_reason: StoppedReason::StalemateDetected,
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
