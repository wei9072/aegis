//! V3.6 — permission policy wired into ConversationRuntime.
//! V3.9 (B3.2) — path-based PermissionRule end-to-end coverage.

use aegis_agent::permission::{
    PermissionMode, PermissionPolicy, PermissionRule, RuleDecision,
};
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
fn plan_mode_dry_runs_edit_without_calling_executor() {
    // ToolExecutor has NO Edit script — if it ran, the call would
    // fail with "no such tool". A successful turn proves plan mode
    // synthesised the result instead of dispatching.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Edit", r#"{"path":"x.py","old_string":"a","new_string":"b"}"#)
        .push_text_then_done("plan looks fine");
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::Plan));

    let result = rt.run_turn("propose an edit");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult {
            output, is_error, ..
        } => {
            assert!(!*is_error, "plan-mode synthesis is a fact-shaped result, not an error");
            assert!(
                output.contains("plan mode") && output.contains("NOT EXECUTED"),
                "expected plan-mode marker; got: {output}"
            );
            assert!(
                output.contains("Edit"),
                "expected tool name in synthesized result; got: {output}"
            );
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn plan_mode_lets_reads_through_normally() {
    // Read tools execute in plan mode — only writes dry-run.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Read", r#"{"path":"x.py"}"#)
        .push_text_then_done("read it");
    let tools = ScriptedToolExecutor::new().with_ok("Read", "file contents");

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::Plan));

    let _ = rt.run_turn("read this");
    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult { output, is_error, .. } => {
            assert!(!*is_error);
            assert_eq!(output, "file contents");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn plan_mode_denies_bash_explicitly() {
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "bash", r#"{"command":"rm -rf /"}"#)
        .push_text_then_done("ok");
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(PermissionPolicy::standard(PermissionMode::Plan));

    let _ = rt.run_turn("danger");
    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult { output, is_error, .. } => {
            assert!(*is_error);
            assert!(output.contains("permission denied") && output.contains("Plan"));
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn rule_denies_edit_in_protected_path_even_under_workspace_write() {
    // Mode = WorkspaceWrite would normally allow Edit; rule overrides
    // for paths matching vendor/**.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Edit", r#"{"path":"vendor/dep.rs","old_string":"a","new_string":"b"}"#)
        .push_text_then_done("blocked, ok");
    let tools = ScriptedToolExecutor::new();

    let policy = PermissionPolicy::standard(PermissionMode::WorkspaceWrite).with_rules(vec![
        PermissionRule {
            tool_glob: "Edit".into(),
            path_glob: Some("vendor/**".into()),
            decision: RuleDecision::Deny,
        },
    ]);

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(policy);

    let _ = rt.run_turn("touch vendor");

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult { output, is_error, .. } => {
            assert!(*is_error);
            assert!(
                output.contains("permission denied by rule"),
                "expected rule-denial message; got: {output}"
            );
            assert!(output.contains("vendor/dep.rs"));
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn rule_prompt_collapses_to_deny_when_no_prompter_configured() {
    // B3.3 — without a PermissionPrompter wired in, Prompt rules
    // safely default to deny.
    let api = ScriptedApiClient::new()
        .push_tool_call("c1", "Edit", r#"{"path":"x.py","old_string":"a","new_string":"b"}"#)
        .push_text_then_done("ok");
    let tools = ScriptedToolExecutor::new();

    let policy = PermissionPolicy::standard(PermissionMode::WorkspaceWrite).with_rules(vec![
        PermissionRule {
            tool_glob: "Edit".into(),
            path_glob: None,
            decision: RuleDecision::Prompt,
        },
    ]);

    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_permission_policy(policy);

    let _ = rt.run_turn("ask permission");

    let tool_msg = &rt.session().messages[2];
    match &tool_msg.blocks[0] {
        ContentBlock::ToolResult { output, is_error, .. } => {
            assert!(*is_error);
            assert!(output.contains("no prompter configured"));
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
