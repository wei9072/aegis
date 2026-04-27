//! V3.6 — permission policy wired into ConversationRuntime.

use aegis_agent::permission::{PermissionMode, PermissionPolicy};
use aegis_agent::testing::{ScriptedApiClient, ScriptedToolExecutor};
use aegis_agent::{
    AgentConfig, ConversationRuntime, ContentBlock, MessageRole, Session, StoppedReason,
};

fn cfg() -> AgentConfig {
    AgentConfig {
        max_iterations_per_turn: 5,
        session_cost_budget: None,
        workspace_root: None,
    }
}

#[test]
fn read_only_mode_denies_write_call_before_executor() {
    // Tool executor would PANIC if called (returns "no such tool"),
    // proving the permission gate short-circuits before reaching it.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Edit", r#"{"path":"x.py"}"#)
        .push_text_then_done("blocked, ok");
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::ReadOnly));

    let result = rt.run_turn("please edit");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);

    let tool_msg = &rt.session().messages[2];
    assert_eq!(tool_msg.role, MessageRole::Tool);
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(*is_error);
            assert!(output.contains("permission denied"));
            assert!(output.contains("ReadOnly"));
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn workspace_write_mode_allows_edit() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Edit", r#"{"path":"x.py"}"#)
        .push_text_then_done("done");
    let tools = ScriptedToolExecutor::new().with_ok("Edit", "edited");

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::WorkspaceWrite));

    let result = rt.run_turn("edit");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(!*is_error);
            assert_eq!(output, "edited");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn workspace_write_denies_bash() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "bash", r#"{"command":"ls"}"#)
        .push_text_then_done("ok no bash");
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::WorkspaceWrite));

    let _ = rt.run_turn("hi");

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(*is_error);
            assert!(output.contains("permission denied"));
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn danger_full_access_allows_bash() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "bash", r#"{"command":"ls"}"#)
        .push_text_then_done("done");
    let tools = ScriptedToolExecutor::new().with_ok("bash", "files...");

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::DangerFullAccess));

    let result = rt.run_turn("run ls");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult { output, .. } => assert_eq!(output, "files..."),
        _ => panic!("expected ToolResult"),
    }
}
