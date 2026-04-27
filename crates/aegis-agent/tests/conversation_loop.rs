//! V3.1 — end-to-end conversation loop tests using scripted stubs.
//!
//! These exercise the borrowed claw-code conversation skeleton end
//! to end without a live LLM:
//!   - Single text response (no tool calls) → `PlanDoneNoVerifier`
//!   - One tool call → execute → done on next round
//!   - Tool execution failure flows back to LLM (does NOT trigger
//!     auto-retry — the runtime returns the next `stream` call's
//!     output verbatim)
//!   - Per-turn iteration budget hit → `MaxIterations`
//!   - Provider stream error → `ProviderError`

use aegis_agent::testing::{ScriptedApiClient, ScriptedToolExecutor};
use aegis_agent::{
    AgentConfig, ConversationRuntime, MessageRole, Session, StoppedReason,
};

fn cfg(max_iters: u32) -> AgentConfig {
    AgentConfig {
        max_iterations_per_turn: max_iters,
        session_cost_budget: None,
    }
}

#[test]
fn single_text_response_terminates_with_plan_done_no_verifier() {
    let api = ScriptedApiClient::new().push_text_then_done("hello back");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5));

    let result = rt.run_turn("hello");

    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
    assert_eq!(result.iterations, 1);
    assert!(result.task_verdict.is_none());

    // Session should now hold: user msg + assistant msg.
    assert_eq!(rt.session().messages.len(), 2);
    assert_eq!(rt.session().messages[0].role, MessageRole::User);
    assert_eq!(rt.session().messages[1].role, MessageRole::Assistant);
}

#[test]
fn tool_call_then_text_response_two_iterations() {
    let api = ScriptedApiClient::new()
        .push_tool_call("call_1", "echo", "{\"text\":\"hi\"}")
        .push_text_then_done("done");
    let tools = ScriptedToolExecutor::new().with_ok("echo", "hi");
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5));

    let result = rt.run_turn("please echo hi");

    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
    assert_eq!(result.iterations, 2);

    // Session: user + assistant(tool_use) + tool_result + assistant(text).
    assert_eq!(rt.session().messages.len(), 4);
    assert_eq!(rt.session().messages[0].role, MessageRole::User);
    assert_eq!(rt.session().messages[1].role, MessageRole::Assistant);
    assert_eq!(rt.session().messages[2].role, MessageRole::Tool);
    assert_eq!(rt.session().messages[3].role, MessageRole::Assistant);
}

#[test]
fn tool_failure_flows_back_to_llm_no_auto_retry() {
    // The runtime does NOT see the tool failure and "retry the same
    // call". It just appends the error as a ToolResult (is_error=true)
    // and asks the LLM what to do next. The LLM in this script chooses
    // to give up with a text response.
    let api = ScriptedApiClient::new()
        .push_tool_call("call_1", "broken_tool", "{}")
        .push_text_then_done("I tried but the tool failed; stopping.");
    let tools = ScriptedToolExecutor::new().with_err("broken_tool", "boom");
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5));

    let result = rt.run_turn("try the broken tool");

    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
    assert_eq!(result.iterations, 2);

    let tool_msg = &rt.session().messages[2];
    assert_eq!(tool_msg.role, MessageRole::Tool);
    let block = &tool_msg.blocks[0];
    match block {
        aegis_agent::ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(*is_error);
            assert_eq!(output, "boom");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn iteration_budget_caps_runaway_tool_loops() {
    // Three tool calls scripted; budget is 2. The runtime should
    // return MaxIterations on the third attempt — NOT silently retry.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "echo", "{}")
        .push_tool_call("c2", "echo", "{}")
        .push_tool_call("c3", "echo", "{}");
    let tools = ScriptedToolExecutor::new().with_ok("echo", "hi");
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(2));

    let result = rt.run_turn("loop forever");

    assert_eq!(result.stopped_reason, StoppedReason::MaxIterations);
    // iterations field reports completed iters before the budget bailed.
    assert_eq!(result.iterations, 2);
    assert!(result.task_verdict.is_none());
}

#[test]
fn provider_error_surfaces_as_stopped_reason_no_retry() {
    let api = ScriptedApiClient::new().push_error("HTTP 503");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg(5));

    let result = rt.run_turn("hi");

    match result.stopped_reason {
        StoppedReason::ProviderError(message) => assert_eq!(message, "HTTP 503"),
        other => panic!("expected ProviderError, got {other:?}"),
    }
    // Provider error does NOT trigger an automatic retry — the agent
    // reports and stops; user (or orchestrator) decides whether to
    // start another turn.
    assert_eq!(result.iterations, 1);
}
