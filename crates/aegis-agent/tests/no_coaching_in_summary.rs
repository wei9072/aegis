//! Contract test — B4.5.
//!
//! Aegis compaction MUST NOT smuggle coaching language into the
//! summary text it writes back to the conversation. This test runs
//! the summary builder against a representative session + cost
//! tracker + verdict history and greps for forbidden coaching words.
//!
//! If a future change adds LLM-generated narrative to the summary
//! (or even templated phrases that drift toward "you should X"),
//! this test fires loudly. The fix is always: remove the prose, fall
//! back to fact-shaped tallies.
//!
//! Runs alongside the sibling contracts:
//!   - tests/no_auto_retry.rs       (no retry loop)
//!   - tests/no_coaching_injection.rs (verifier verdicts not coached)
//!   - tests/verifier_drives_done.rs  (LLM-claimed done not authoritative)

use aegis_agent::compact::{build_summary, compact_session, CompactionConfig};
use aegis_agent::cost::CostTracker;
use aegis_agent::message::{ContentBlock, ConversationMessage, MessageRole, Session};
use aegis_decision::{TaskPattern, TaskVerdict, VerifierResult};
use serde_json::json;

/// Coaching words / phrases the summary must never contain. Lower-cased
/// match. Comments capture the rationale so future contributors don't
/// remove them on a "looks fine" basis.
const FORBIDDEN_COACHING_PHRASES: &[&str] = &[
    // Direct second-person directives
    "you should",
    "you must",
    "you need to",
    "you can",
    "please ",
    // Hedged advice
    "consider ",
    "try to",
    "maybe try",
    "we recommend",
    "it's recommended",
    "suggest",
    // Forward-looking direction
    "next step",
    "next steps",
    "going forward",
    "moving forward",
    // Past-attempt framing that becomes coaching
    "instead of",
    "instead, ",
    "rather than",
    // Common LLM hedges that slip in via prose summaries
    "carefully",
    "ideally",
    "make sure to",
    "be sure to",
    "remember to",
];

fn build_test_session() -> Session {
    let mut s = Session::new();
    for i in 0..6 {
        s.push(ConversationMessage::user_text(format!("turn {i} input")));
        s.push(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: format!("call_{i}"),
                name: if i % 2 == 0 { "Edit" } else { "Read" }.into(),
                input: json!({ "path": format!("src/file_{i}.rs") }).to_string(),
            }],
        });
        s.push(ConversationMessage::tool_result(
            &format!("call_{i}"),
            "Edit",
            "ok",
            false,
        ));
    }
    s
}

fn build_test_cost_tracker() -> CostTracker {
    let mut t = CostTracker::new();
    t.observe("src/file_0.rs", 8.0);
    t.observe("src/file_0.rs", 12.0);
    t.observe("src/file_2.rs", 10.0);
    t.observe("src/file_2.rs", 9.0);
    t
}

fn build_test_verdicts() -> Vec<TaskVerdict> {
    vec![
        TaskVerdict {
            pattern: TaskPattern::Solved,
            ..TaskVerdict::no_verifier(true, 1)
        },
        TaskVerdict {
            pattern: TaskPattern::Incomplete,
            verifier_result: Some(VerifierResult {
                passed: false,
                rationale: "tests failed".into(),
                evidence: Default::default(),
            }),
            ..TaskVerdict::no_verifier(false, 2)
        },
    ]
}

#[test]
fn build_summary_emits_no_coaching_phrase() {
    let session = build_test_session();
    let cost = build_test_cost_tracker();
    let verdicts = build_test_verdicts();
    let summary = build_summary(&session.messages, &cost, &verdicts);
    let lc = summary.to_lowercase();
    for phrase in FORBIDDEN_COACHING_PHRASES {
        assert!(
            !lc.contains(phrase),
            "summary leaked coaching phrase {phrase:?}.\n\
             FULL SUMMARY:\n{summary}"
        );
    }
}

#[test]
fn compact_session_writes_no_coaching_into_replacement_message() {
    let mut session = build_test_session();
    let cost = build_test_cost_tracker();
    let verdicts = build_test_verdicts();
    let _ = compact_session(
        &mut session,
        &cost,
        &verdicts,
        &CompactionConfig { keep_last_turns: 1 },
    )
    .expect("session has 6 turns, keep 1, should compact 5");

    // The first message is now the [aegis-compacted] summary block.
    let first = &session.messages[0];
    let text = match &first.blocks[0] {
        ContentBlock::Text { text } => text.clone(),
        other => panic!("expected Text block, got {other:?}"),
    };
    let lc = text.to_lowercase();
    for phrase in FORBIDDEN_COACHING_PHRASES {
        assert!(
            !lc.contains(phrase),
            "compacted message leaked coaching phrase {phrase:?}.\n\
             FULL TEXT:\n{text}"
        );
    }
}

#[test]
fn build_summary_only_uses_section_label_keywords() {
    // Defence in depth: even the section labels must stay
    // observation-shaped. If a future contributor renames a section
    // header to "next steps" or similar, the previous test catches
    // it; this one pins the actual wording.
    let session = build_test_session();
    let summary = build_summary(&session.messages, &CostTracker::new(), &[]);
    assert!(summary.contains("(a) facts"));
    assert!(summary.contains("(b) files_touched"));
    assert!(summary.contains("(c) cost_trajectory"));
    assert!(summary.contains("(d) verifier_verdicts"));
}
