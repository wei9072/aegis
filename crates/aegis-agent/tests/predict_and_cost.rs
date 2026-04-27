//! V3.3 — PreToolUse aegis-predict + cross-turn cost tracking.
//!
//! Verifies the runtime hooks fire at the right places and produce
//! the right end-to-end behaviour, without depending on a real
//! aegis-mcp subprocess.

use aegis_agent::conversation::{ConversationRuntime, CostObserver};
use aegis_agent::predict::{PreToolUsePredictor, PredictVerdict};
use aegis_agent::testing::{ScriptedApiClient, ScriptedToolExecutor};
use aegis_agent::{
    AgentConfig, ContentBlock, MessageRole, Session, StoppedReason,
};
use std::path::PathBuf;

fn cfg(max_iters: u32, budget: Option<f64>) -> AgentConfig {
    AgentConfig {
        max_iterations_per_turn: max_iters,
        session_cost_budget: budget,
    }
}

// ---------- predictor: BLOCK short-circuits without calling tool executor ----------

struct AlwaysBlockPredictor;
impl PreToolUsePredictor for AlwaysBlockPredictor {
    fn predict(&mut self, _tool_name: &str, _input: &str) -> PredictVerdict {
        PredictVerdict::Block {
            reason: "predicted bad".into(),
        }
    }
}

#[test]
fn predictor_block_skips_tool_execution_and_surfaces_reason_to_llm() {
    // The LLM asks for a tool call, then on round 2 acknowledges the
    // block and ends the turn.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Edit", r#"{"path":"x.py","new_content":"x=1"}"#)
        .push_text_then_done("blocked, stopping");
    // Tool executor would PANIC if called — we use empty executor
    // so any execution would fail. The predictor must short-circuit.
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5, None))
        .with_predictor(Box::new(AlwaysBlockPredictor));

    let result = rt.run_turn("please edit");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
    assert_eq!(result.iterations, 2);

    // The tool_result message must be is_error=true with the predict
    // reason as the body.
    let messages = &rt.session().messages;
    let tool_msg = &messages[2];
    assert_eq!(tool_msg.role, MessageRole::Tool);
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(*is_error);
            assert_eq!(output, "predicted bad");
        }
        _ => panic!("expected ToolResult"),
    }
}

// ---------- predictor: Allow lets execution proceed normally ----------

struct AllowAllPredictor;
impl PreToolUsePredictor for AllowAllPredictor {
    fn predict(&mut self, _: &str, _: &str) -> PredictVerdict {
        PredictVerdict::Allow
    }
}

#[test]
fn predictor_allow_passes_through_to_normal_execution() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "echo", "{}")
        .push_text_then_done("done");
    let tools = ScriptedToolExecutor::new().with_ok("echo", "result");
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5, None))
        .with_predictor(Box::new(AllowAllPredictor));

    let result = rt.run_turn("call echo");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(!*is_error);
            assert_eq!(output, "result");
        }
        _ => panic!("expected ToolResult"),
    }
}

// ---------- cost observer + budget enforcement ----------

struct ScriptedCostObserver {
    pub script: Vec<Vec<(PathBuf, f64)>>,
    pub call_index: usize,
}

impl CostObserver for ScriptedCostObserver {
    fn observe(
        &mut self,
        _tool_name: &str,
        _input: &str,
    ) -> Vec<(PathBuf, f64)> {
        if self.call_index >= self.script.len() {
            return Vec::new();
        }
        let observations = self.script[self.call_index].clone();
        self.call_index += 1;
        observations
    }
}

#[test]
fn cost_tracker_records_observations_each_turn() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "edit", r#"{"path":"a.py","new_content":"x"}"#)
        .push_text_then_done("done");
    let tools = ScriptedToolExecutor::new().with_ok("edit", "ok");
    let observer = Box::new(ScriptedCostObserver {
        script: vec![vec![(PathBuf::from("a.py"), 5.0)]],
        call_index: 0,
    });

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5, None))
        .with_cost_observer(observer);

    let _ = rt.run_turn("edit");

    let snapshot = rt.cost_tracker().snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].path, PathBuf::from("a.py"));
    assert_eq!(snapshot[0].current, 5.0);
    assert_eq!(snapshot[0].baseline, 5.0);
}

#[test]
fn cost_budget_exceeded_terminates_session_no_retry() {
    // Three tool calls. After the second, cumulative regression
    // crosses the budget of 5.0 → session terminates.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "edit", r#"{"path":"a.py"}"#)
        .push_tool_call("c2", "edit", r#"{"path":"a.py"}"#)
        .push_tool_call("c3", "edit", r#"{"path":"a.py"}"#);
    let tools = ScriptedToolExecutor::new().with_ok("edit", "ok");

    // First obs sets baseline at 10. Second pushes to 14 (regression=4).
    // Third would push to 20 (regression=10) — budget=5 → blow.
    let observer = Box::new(ScriptedCostObserver {
        script: vec![
            vec![(PathBuf::from("a.py"), 10.0)],
            vec![(PathBuf::from("a.py"), 14.0)],
            vec![(PathBuf::from("a.py"), 20.0)],
        ],
        call_index: 0,
    });

    let mut rt = ConversationRuntime::new(
        Session::new(),
        api,
        tools,
        vec![],
        vec![],
        cfg(10, Some(5.0)),
    )
    .with_cost_observer(observer);

    let result = rt.run_turn("burn budget");
    assert_eq!(result.stopped_reason, StoppedReason::CostBudgetExceeded);
    assert!(result.iterations >= 1);
    assert!(rt.cost_tracker().cumulative_regression() > 5.0);
}

#[test]
fn improvement_within_budget_does_not_terminate() {
    // Edit makes file better — regression = 0.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "edit", r#"{"path":"a.py"}"#)
        .push_text_then_done("done");
    let tools = ScriptedToolExecutor::new().with_ok("edit", "ok");
    let observer = Box::new(ScriptedCostObserver {
        script: vec![vec![(PathBuf::from("a.py"), 3.0)]],
        call_index: 0,
    });

    let mut rt = ConversationRuntime::new(
        Session::new(),
        api,
        tools,
        vec![],
        vec![],
        cfg(5, Some(0.5)),
    )
    .with_cost_observer(observer);

    let result = rt.run_turn("improve");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
}

// ---------- failed tool execution does not feed the cost observer ----------

#[test]
fn tool_failure_skips_cost_observation() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "broken", "{}")
        .push_text_then_done("ok stopping");
    let tools = ScriptedToolExecutor::new().with_err("broken", "boom");
    let observer = Box::new(ScriptedCostObserver {
        script: vec![vec![(PathBuf::from("would.py"), 99.0)]],
        call_index: 0,
    });

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5, None))
        .with_cost_observer(observer);

    let _ = rt.run_turn("hi");
    assert!(
        rt.cost_tracker().snapshot().is_empty(),
        "tool failure must not feed cost observer"
    );
}
