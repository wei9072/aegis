//! Session compaction (B4 / Phase 4).
//!
//! When a long session approaches the model's context window the
//! conversation runtime trims early messages and replaces them with
//! a structured summary. The runtime keeps recent turns intact so
//! short-term context survives.
//!
//! ## Negative-space framing — why this is built differently
//!
//! claw-code's compaction asks the LLM to write a prose summary of
//! the early conversation. That summary is whatever the LLM says it
//! is — and it routinely smuggles in coaching ("the previous attempt
//! failed because of X, you should try Y") which then leaks into the
//! next turn's prompt as if it were established fact.
//!
//! Aegis compaction is **constructed from observed state**, not from
//! LLM narrative:
//!
//!   (a) **Facts established** — message + tool-call counts, total
//!       turns, tool-call breakdown by name. Pure tallies.
//!   (b) **Files touched** — distinct path list pulled from tool
//!       inputs (Edit / Write / Read). Path strings only, no
//!       narrative.
//!   (c) **Cost trajectory** — baseline vs current per file from the
//!       cost tracker. Pure numbers from Ring 0.5 signals.
//!   (d) **Verifier verdict history** — PASS / INCOMPLETE timeline.
//!       Pure verdict labels.
//!
//! No LLM is invoked. The summary is deterministic given the input
//! state. The contract test `tests/no_coaching_in_summary.rs` greps
//! the produced summary for forbidden coaching words and fails the
//! build if any leak in.

use crate::cost::CostTracker;
use crate::message::{ContentBlock, ConversationMessage, MessageRole};
use aegis_decision::TaskVerdict;
#[cfg(test)]
use aegis_decision::TaskPattern;

/// Knobs for `compact_session`. All sensible defaults — most callers
/// just call `..Default::default()`.
#[derive(Clone, Debug)]
pub struct CompactionConfig {
    /// Keep this many of the most recent turns intact. Everything
    /// before that gets summarized. Default 5.
    pub keep_last_turns: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            keep_last_turns: 5,
        }
    }
}

/// Outcome of a single compaction pass.
#[derive(Clone, Debug)]
pub struct CompactionResult {
    /// The structured summary text. Inserted into the session as a
    /// single user-role message replacing the early ones.
    pub summary: String,
    /// Number of original messages collapsed into the summary.
    pub messages_compacted: usize,
}

/// Compact a session in place. Returns `None` when there's nothing
/// to compact (session shorter than `keep_last_turns` worth of
/// turns). On success, the session's `messages` vec ends up as:
///
///   [summary_message, ...last_keep_turns]
///
/// Caller is responsible for re-fitting the result to the model's
/// context window — this function makes the summary, it doesn't
/// estimate tokens.
pub fn compact_session(
    session: &mut crate::message::Session,
    cost_tracker: &CostTracker,
    verifier_history: &[TaskVerdict],
    config: &CompactionConfig,
) -> Option<CompactionResult> {
    let cutoff = pick_cutoff(&session.messages, config.keep_last_turns)?;
    if cutoff == 0 {
        return None;
    }

    let early: Vec<ConversationMessage> = session.messages.drain(..cutoff).collect();
    let summary = build_summary(&early, cost_tracker, verifier_history);
    let summary_msg = ConversationMessage::user_text(format!("[aegis-compacted]\n{summary}"));

    // Push summary to the FRONT of what's now in the session.
    session.messages.insert(0, summary_msg);

    Some(CompactionResult {
        summary,
        messages_compacted: cutoff,
    })
}

/// Find the index in `messages` such that `messages[..cutoff]` is
/// what should be summarized and `messages[cutoff..]` is preserved
/// verbatim. A "turn" boundary is each user-role message — we count
/// backwards from the end and pick the start of the Nth-from-last
/// turn. Returns `None` when there aren't enough turns to bother
/// compacting.
fn pick_cutoff(messages: &[ConversationMessage], keep_last_turns: usize) -> Option<usize> {
    let user_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == MessageRole::User)
        .map(|(i, _)| i)
        .collect();
    if user_indices.len() <= keep_last_turns {
        return None;
    }
    // Index of the user message that starts the kept tail.
    let tail_start = user_indices[user_indices.len() - keep_last_turns];
    Some(tail_start)
}

/// The four-column structured summary. Pure function over its
/// inputs — same inputs always produce the same output, byte-for-byte.
#[must_use]
pub fn build_summary(
    early: &[ConversationMessage],
    cost_tracker: &CostTracker,
    verifier_history: &[TaskVerdict],
) -> String {
    let counts = compute_counts(early);
    let files = collect_touched_files(early);
    let cost_lines = format_cost_trajectory(cost_tracker);
    let verdict_lines = format_verdict_history(verifier_history);

    let mut out = String::with_capacity(512);
    out.push_str("(a) facts\n");
    out.push_str(&format!("    user_messages={}\n", counts.user_msgs));
    out.push_str(&format!("    assistant_messages={}\n", counts.assistant_msgs));
    out.push_str(&format!("    tool_results={}\n", counts.tool_results));
    out.push_str(&format!("    tool_calls={}\n", counts.tool_calls));
    if !counts.tool_call_breakdown.is_empty() {
        out.push_str("    tool_call_breakdown:\n");
        for (name, n) in &counts.tool_call_breakdown {
            out.push_str(&format!("      {name}={n}\n"));
        }
    }

    out.push_str("\n(b) files_touched\n");
    if files.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for f in &files {
            out.push_str(&format!("    {f}\n"));
        }
    }

    out.push_str("\n(c) cost_trajectory\n");
    if cost_lines.is_empty() {
        out.push_str("    (no cost observations)\n");
    } else {
        for line in &cost_lines {
            out.push_str(&format!("    {line}\n"));
        }
    }

    out.push_str("\n(d) verifier_verdicts\n");
    if verdict_lines.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for line in &verdict_lines {
            out.push_str(&format!("    {line}\n"));
        }
    }

    out
}

#[derive(Default)]
struct MessageCounts {
    user_msgs: usize,
    assistant_msgs: usize,
    tool_results: usize,
    tool_calls: usize,
    tool_call_breakdown: std::collections::BTreeMap<String, usize>,
}

fn compute_counts(messages: &[ConversationMessage]) -> MessageCounts {
    let mut c = MessageCounts::default();
    for msg in messages {
        match msg.role {
            MessageRole::User => c.user_msgs += 1,
            MessageRole::Assistant => c.assistant_msgs += 1,
            MessageRole::Tool => c.tool_results += 1,
            MessageRole::System => {}
        }
        for block in &msg.blocks {
            if let ContentBlock::ToolUse { name, .. } = block {
                c.tool_calls += 1;
                *c.tool_call_breakdown.entry(name.clone()).or_insert(0) += 1;
            }
        }
    }
    c
}

fn collect_touched_files(messages: &[ConversationMessage]) -> Vec<String> {
    let mut paths = std::collections::BTreeSet::new();
    for msg in messages {
        for block in &msg.blocks {
            if let ContentBlock::ToolUse { input, .. } = block {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(input) {
                    if let Some(p) = value.get("path").and_then(|v| v.as_str()) {
                        paths.insert(p.to_string());
                    }
                }
            }
        }
    }
    paths.into_iter().collect()
}

fn format_cost_trajectory(tracker: &CostTracker) -> Vec<String> {
    let snapshot = tracker.snapshot();
    if snapshot.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = snapshot
        .iter()
        .map(|e| {
            format!(
                "{}: baseline={:.0} current={:.0} regression={:.0}",
                e.path.display(),
                e.baseline,
                e.current,
                e.regression()
            )
        })
        .collect();
    lines.push(format!(
        "cumulative_regression={:.0}",
        tracker.cumulative_regression()
    ));
    lines
}

fn format_verdict_history(verdicts: &[TaskVerdict]) -> Vec<String> {
    verdicts
        .iter()
        .enumerate()
        .map(|(i, v)| format!("turn {i}: {}", v.pattern.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Session;
    use serde_json::json;

    fn user(text: &str) -> ConversationMessage {
        ConversationMessage::user_text(text)
    }

    fn assistant_with_tool(name: &str, path: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "id".into(),
                name: name.into(),
                input: json!({ "path": path }).to_string(),
            }],
        }
    }

    fn tool_result(text: &str) -> ConversationMessage {
        ConversationMessage::tool_result("id", "X", text, false)
    }

    #[test]
    fn pick_cutoff_returns_none_when_session_is_short() {
        let messages = vec![user("a"), user("b"), user("c")];
        assert_eq!(pick_cutoff(&messages, 5), None);
    }

    #[test]
    fn pick_cutoff_keeps_last_n_user_messages_intact() {
        let messages = vec![
            user("turn 0"),
            assistant_with_tool("Edit", "a.rs"),
            tool_result("ok"),
            user("turn 1"),
            assistant_with_tool("Edit", "b.rs"),
            tool_result("ok"),
            user("turn 2"),
            assistant_with_tool("Edit", "c.rs"),
            tool_result("ok"),
        ];
        // Keep last 1 turn → cutoff at index of last user message.
        let cut = pick_cutoff(&messages, 1).unwrap();
        assert_eq!(cut, 6);
    }

    #[test]
    fn build_summary_includes_all_four_sections() {
        let messages = vec![user("hi"), assistant_with_tool("Edit", "x.rs")];
        let cost = CostTracker::new();
        let s = build_summary(&messages, &cost, &[]);
        assert!(s.contains("(a) facts"));
        assert!(s.contains("(b) files_touched"));
        assert!(s.contains("(c) cost_trajectory"));
        assert!(s.contains("(d) verifier_verdicts"));
    }

    #[test]
    fn build_summary_counts_tool_calls_per_name() {
        let messages = vec![
            assistant_with_tool("Edit", "a.rs"),
            assistant_with_tool("Edit", "b.rs"),
            assistant_with_tool("Read", "c.rs"),
        ];
        let s = build_summary(&messages, &CostTracker::new(), &[]);
        assert!(s.contains("Edit=2"));
        assert!(s.contains("Read=1"));
    }

    #[test]
    fn build_summary_collects_unique_paths() {
        let messages = vec![
            assistant_with_tool("Edit", "a.rs"),
            assistant_with_tool("Edit", "a.rs"), // duplicate
            assistant_with_tool("Read", "b.rs"),
        ];
        let s = build_summary(&messages, &CostTracker::new(), &[]);
        // BTreeSet → distinct paths. "a.rs" appears once, "b.rs" once.
        let count_a = s.matches("a.rs").count();
        let count_b = s.matches("b.rs").count();
        assert_eq!(count_a, 1);
        assert_eq!(count_b, 1);
    }

    #[test]
    fn build_summary_emits_cost_trajectory_when_tracker_populated() {
        let mut cost = CostTracker::new();
        cost.observe("foo.rs", 10.0);
        cost.observe("foo.rs", 15.0);
        let s = build_summary(&[], &cost, &[]);
        assert!(s.contains("foo.rs"));
        assert!(s.contains("baseline=10 current=15 regression=5"));
        assert!(s.contains("cumulative_regression=5"));
    }

    #[test]
    fn build_summary_emits_verdict_timeline() {
        let v_solved = TaskVerdict::no_verifier(true, 1);
        // Replace the no_verifier pattern with Solved/Incomplete for
        // a more representative sample.
        let v_solved = TaskVerdict {
            pattern: TaskPattern::Solved,
            ..v_solved
        };
        let v_incomplete = TaskVerdict {
            pattern: TaskPattern::Incomplete,
            ..TaskVerdict::no_verifier(false, 1)
        };
        let s = build_summary(&[], &CostTracker::new(), &[v_solved, v_incomplete]);
        assert!(s.contains("turn 0: solved"));
        assert!(s.contains("turn 1: incomplete"));
    }

    #[test]
    fn compact_session_replaces_early_messages_with_summary() {
        let mut sess = Session::new();
        for i in 0..6 {
            sess.push(user(&format!("turn {i}")));
            sess.push(assistant_with_tool("Edit", &format!("f{i}.rs")));
            sess.push(tool_result("ok"));
        }
        let original_len = sess.messages.len();
        assert_eq!(original_len, 18);

        let result = compact_session(
            &mut sess,
            &CostTracker::new(),
            &[],
            &CompactionConfig { keep_last_turns: 2 },
        )
        .expect("should compact");

        // 4 turns → 12 messages compacted; 2 turns × 3 = 6 + 1 summary = 7.
        assert_eq!(result.messages_compacted, 12);
        assert_eq!(sess.messages.len(), 7);
        assert_eq!(sess.messages[0].role, MessageRole::User);
        match &sess.messages[0].blocks[0] {
            ContentBlock::Text { text } => {
                assert!(text.starts_with("[aegis-compacted]"));
                assert!(text.contains("(a) facts"));
            }
            _ => panic!("expected Text block in summary message"),
        }
    }

    #[test]
    fn compact_session_returns_none_when_session_too_short() {
        let mut sess = Session::new();
        sess.push(user("only turn"));
        let r = compact_session(
            &mut sess,
            &CostTracker::new(),
            &[],
            &CompactionConfig::default(),
        );
        assert!(r.is_none());
        assert_eq!(sess.messages.len(), 1);
    }
}
