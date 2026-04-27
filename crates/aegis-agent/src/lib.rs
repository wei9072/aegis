//! aegis-agent — a coding agent built on aegis primitives.
//!
//! V3 design — substrate + hand. aegis-agent borrows
//! conversation/tool/api scaffolding from claw-code (MIT) and adds
//! four aegis-specific differentiation points:
//!
//!   1. PreToolUse aegis-verdict prediction (agent self-rejects bad plans)
//!   2. Cross-turn structural cost tracking
//!   3. Verifier-driven done (LLM cannot single-handedly claim "done")
//!   4. Stalemate / thrashing detection at session level
//!
//! Negative-space framing rules — structurally enforced by `tests/`:
//!
//!   - **No auto-retry.** Agent runs a turn; if anything fails
//!     (verifier INCOMPLETE, stalemate, etc.), the agent reports
//!     and stops. The user (or upstream orchestrator) decides
//!     whether to start a new session.
//!
//!   - **No coaching injection.** Verifier verdicts go to the user
//!     as observation; never get turned into hint strings injected
//!     into the next prompt.
//!
//!   - **Verifier overrules LLM-claimed done.** When the LLM emits
//!     no more tool_use blocks (a "done" signal), the agent runs
//!     the configured verifier; the verifier's verdict is the final
//!     word on whether the turn truly completed.
//!
//! See `docs/v3_agent_design.md` for the design rationale, and
//! `crates/aegis-decision/src/task.rs::tests::task_verdict_has_no_feedback_field`
//! for the sibling contract guarding `TaskVerdict`.

pub mod aegis_predict;
pub mod api;
pub mod conversation;
pub mod cost;
pub mod mcp;
pub mod message;
pub mod predict;
pub mod providers;
pub mod testing;
pub mod tool;

pub use api::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolDefinition};
pub use conversation::ConversationRuntime;
pub use message::{ContentBlock, ConversationMessage, MessageRole, Session};
pub use tool::{ToolError, ToolExecutor};

use aegis_decision::TaskVerdict;
use serde::{Deserialize, Serialize};

/// Agent configuration. Note the deliberate absence of any
/// retry / auto-retry / feedback-injection toggles — those would
/// violate the negative-space framing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Maximum tool-use iterations within a single turn before the
    /// turn terminates with `StoppedReason::MaxIterations`.
    /// (This is a budget guard, not a retry. The user starts a new
    /// turn — the agent does not re-invoke itself.)
    pub max_iterations_per_turn: u32,

    /// Cost-regression threshold across a session. When cumulative
    /// structural cost exceeds this, the session terminates with
    /// `StoppedReason::CostBudgetExceeded`.
    pub session_cost_budget: Option<f64>,
}

/// Why a turn ended. Every variant is observation, not direction —
/// none of them implies "and the agent will retry".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoppedReason {
    /// LLM emitted no more tool_use; verifier (if any) agreed.
    PlanDoneVerified,
    /// LLM emitted no more tool_use; verifier disagreed (INCOMPLETE).
    PlanDoneVerifierRejected,
    /// LLM emitted no more tool_use; no verifier configured.
    PlanDoneNoVerifier,
    /// Per-turn iteration budget hit.
    MaxIterations,
    /// Session-level structural-cost budget exceeded.
    CostBudgetExceeded,
    /// Cross-turn stalemate detected (no movement on signals).
    StalemateDetected,
    /// Cross-turn thrashing detected (oscillating cost).
    ThrashingDetected,
    /// API / provider error (not a retry trigger; just reported).
    ProviderError(String),
}

/// Result of one agent turn. By design, this struct contains no
/// field that an outer loop could read to "retry with hint".
#[derive(Clone, Debug)]
pub struct AgentTurnResult {
    pub stopped_reason: StoppedReason,
    pub iterations: u32,
    /// Verifier verdict, if a verifier was configured. Verifier
    /// verdicts are observations, never coaching.
    pub task_verdict: Option<TaskVerdict>,
}
