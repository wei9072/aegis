//! V3.5 — stalemate detection wired into the conversation loop.
//!
//! Verifies that 3 successive iterations with the same cost-total
//! terminate the turn with `StoppedReason::StalemateDetected`,
//! not with `MaxIterations` and not with auto-retry.

use aegis_agent::conversation::CostObserver;
use aegis_agent::testing::{ScriptedApiClient, ScriptedToolExecutor};
use aegis_agent::{AgentConfig, ConversationRuntime, Session, StoppedReason};
use std::path::PathBuf;

fn cfg(max_iters: u32) -> AgentConfig {
    AgentConfig {
        max_iterations_per_turn: max_iters,
        session_cost_budget: None,
        workspace_root: None,
    }
}

struct ConstantCostObserver(f64);
impl CostObserver for ConstantCostObserver {
    fn observe(&mut self, _: &str, _: &str) -> Vec<(PathBuf, f64)> {
        vec![(PathBuf::from("a.py"), self.0)]
    }
}

#[test]
fn three_identical_cost_iterations_trigger_stalemate() {
    // LLM keeps requesting the same tool; cost stays at 10.0 across
    // three iterations → stalemate verdict triggers terminate.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "edit", "{}")
        .push_tool_call("c2", "edit", "{}")
        .push_tool_call("c3", "edit", "{}")
        .push_tool_call("c4", "edit", "{}");
    let tools = ScriptedToolExecutor::new().with_ok("edit", "ok");
    let observer = Box::new(ConstantCostObserver(10.0));

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(10))
        .with_cost_observer(observer);

    let result = rt.run_turn("loop");
    assert_eq!(result.stopped_reason, StoppedReason::StalemateDetected);
    assert!(
        result.iterations <= 3,
        "stalemate must fire by iter 3, got {}",
        result.iterations
    );
}

struct StepCostObserver {
    pub seq: Vec<f64>,
    pub i: usize,
}
impl CostObserver for StepCostObserver {
    fn observe(&mut self, _: &str, _: &str) -> Vec<(PathBuf, f64)> {
        let cost = self.seq.get(self.i).copied().unwrap_or(0.0);
        self.i += 1;
        vec![(PathBuf::from("a.py"), cost)]
    }
}

#[test]
fn movement_in_cost_resets_stalemate_window() {
    // Cost: 10, 10, 11, 11, 11 → on the 5th observation we should
    // stalemate (3 in a row at 11). The first two 10s do NOT
    // trigger because the run of identicals was broken by 11.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "edit", "{}")
        .push_tool_call("c2", "edit", "{}")
        .push_tool_call("c3", "edit", "{}")
        .push_tool_call("c4", "edit", "{}")
        .push_tool_call("c5", "edit", "{}")
        .push_tool_call("c6", "edit", "{}");
    let tools = ScriptedToolExecutor::new().with_ok("edit", "ok");
    let observer = Box::new(StepCostObserver {
        seq: vec![10.0, 10.0, 11.0, 11.0, 11.0, 11.0],
        i: 0,
    });

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(10))
        .with_cost_observer(observer);

    let result = rt.run_turn("loop");
    assert_eq!(result.stopped_reason, StoppedReason::StalemateDetected);
}
