//! Typed reason builders (S3.1) + structured-payload helpers (S3.2).
//!
//! Wire format remains `Vec<serde_json::Value>` for backward
//! compatibility, but every site that pushes a reason now goes
//! through one of the typed constructors below. This gives the
//! verdict consumer two new guarantees:
//!
//! 1. Every reason has the same field layout (`ring`, `verdict_model`,
//!    `decision`, `rule_id`, `detail`, optional `range`, optional
//!    `structured`). Downstream callers (LLM agents, dashboards)
//!    can branch reliably on `ring + verdict_model` instead of
//!    parsing free-form `layer` strings.
//! 2. Structured payloads (cycle paths, lost-symbol caller maps,
//!    rule documentation links) ride on the `structured` field
//!    without ever stuffing them into the human-readable `detail`
//!    string. Keeps the discipline: aegis describes facts, agents
//!    decide fixes.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Which protective layer raised this reason.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ring {
    /// Syntax — tree-sitter parse must succeed.
    Ring0,
    /// Structural signals — fan_out, chain_depth, smell counters.
    Ring0_5,
    /// Security anti-patterns — boolean violations.
    Ring0_7,
    /// Workspace-level — cycles, public symbol loss.
    RingR2,
    /// Cost regression compared against an old baseline.
    Regression,
    /// Other / pre-flight (tempfile errors, unsupported extension).
    Meta,
}

impl Ring {
    pub fn as_str(self) -> &'static str {
        match self {
            Ring::Ring0 => "ring0",
            Ring::Ring0_5 => "ring0_5",
            Ring::Ring0_7 => "ring0_7",
            Ring::RingR2 => "ringR2",
            Ring::Regression => "regression",
            Ring::Meta => "meta",
        }
    }
}

/// How does this reason compute its verdict?
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictModel {
    /// Always-bad pattern; verdict ignores baseline.
    Absolute,
    /// Bad only when worse than the baseline (`old_content` required).
    Delta,
    /// Cannot judge (e.g. unsupported file type).
    NotApplicable,
}

impl VerdictModel {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictModel::Absolute => "absolute",
            VerdictModel::Delta => "delta",
            VerdictModel::NotApplicable => "not_applicable",
        }
    }
}

/// Source-byte range a reason points at.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Range {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl Range {
    pub fn point(line: usize, col: usize) -> Self {
        Self {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }
}

/// Build a typed reason and convert it to wire JSON. The output
/// shape stays additive — older clients ignore the new fields, new
/// clients can branch on `ring`/`verdict_model`/`structured`.
pub fn reason(
    ring: Ring,
    model: VerdictModel,
    decision: &str,
    rule_id: &str,
    detail: impl Into<String>,
    range: Option<Range>,
    structured: Option<Value>,
) -> Value {
    let mut v = json!({
        "layer": ring.as_str(),               // legacy field retained
        "ring": ring.as_str(),                // S3.1 typed field
        "verdict_model": model.as_str(),      // S3.1 typed field
        "decision": decision,
        "reason": rule_id,                    // legacy field name
        "rule_id": rule_id,                   // S3.1 typed field
        "detail": detail.into(),
    });
    if let Some(r) = range {
        v["range"] = json!({
            "start_line": r.start_line,
            "start_col": r.start_col,
            "end_line": r.end_line,
            "end_col": r.end_col,
        });
    }
    if let Some(s) = structured {
        v["structured"] = s;
    }
    v
}

// ─── Convenience constructors per layer ─────────────────────────

pub fn ring0_violation(message: impl Into<String>, range: Range, node_kind: &str) -> Value {
    let mut v = reason(
        Ring::Ring0,
        VerdictModel::Absolute,
        "block",
        "ring0_violation",
        message,
        Some(range),
        None,
    );
    v["node_kind"] = json!(node_kind);
    v
}

pub fn ring0_meta(rule_id: &str, decision: &str, detail: impl Into<String>) -> Value {
    reason(
        Ring::Meta,
        VerdictModel::NotApplicable,
        decision,
        rule_id,
        detail,
        None,
        None,
    )
}

pub fn ring0_5_signal_failure(detail: impl Into<String>) -> Value {
    reason(
        Ring::Ring0_5,
        VerdictModel::Absolute,
        "block",
        "signal_extraction_failed",
        detail,
        None,
        None,
    )
}

pub fn ring0_7_security(rule_id: &str, severity: &str, message: impl Into<String>, range: Range) -> Value {
    let doc_url = format!("https://aegis.dev/rules/{}", rule_id.to_lowercase());
    reason(
        Ring::Ring0_7,
        VerdictModel::Absolute,
        severity,
        rule_id,
        message,
        Some(range),
        Some(json!({ "rule_doc_url": doc_url })),
    )
}

pub fn regression_signal_regressed(growers: &Value, shrinkers: &Value) -> Value {
    reason(
        Ring::Regression,
        VerdictModel::Delta,
        "block",
        "signal_regressed",
        format!("regressed: {growers}; improved: {shrinkers}"),
        None,
        Some(json!({
            "regressed": growers,
            "improved": shrinkers,
        })),
    )
}

pub fn ringR2_cycle_introduced(cycle_path: Vec<String>, file: &str) -> Value {
    reason(
        Ring::RingR2,
        VerdictModel::Delta,
        "block",
        "cycle_introduced",
        format!("change to {file:?} would create a module import cycle"),
        None,
        Some(json!({ "cycle_path": cycle_path, "file": file })),
    )
}

pub fn ringR2_public_symbol_removed(
    removed: &[String],
    callers: serde_json::Map<String, Value>,
) -> Value {
    reason(
        Ring::RingR2,
        VerdictModel::Delta,
        "block",
        "public_symbol_removed",
        format!(
            "removed public symbols still referenced by other files: {removed:?}"
        ),
        None,
        Some(json!({
            "removed": removed,
            "callers": callers,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_reason_carries_ring_and_model() {
        let v = reason(
            Ring::Ring0_7,
            VerdictModel::Absolute,
            "block",
            "SEC003",
            "TLS off",
            Some(Range::point(5, 1)),
            None,
        );
        assert_eq!(v["ring"], "ring0_7");
        assert_eq!(v["verdict_model"], "absolute");
        assert_eq!(v["rule_id"], "SEC003");
        assert_eq!(v["range"]["start_line"], 5);
        // legacy fields still present
        assert_eq!(v["layer"], "ring0_7");
        assert_eq!(v["reason"], "SEC003");
    }

    #[test]
    fn ring0_7_security_attaches_rule_doc_url() {
        let v = ring0_7_security("SEC001", "block", "eval", Range::point(1, 1));
        assert!(v["structured"]["rule_doc_url"]
            .as_str()
            .unwrap()
            .contains("sec001"));
    }

    #[test]
    fn ringR2_cycle_carries_path() {
        let v = ringR2_cycle_introduced(vec!["a.py".into(), "b.py".into(), "a.py".into()], "a.py");
        assert_eq!(v["structured"]["cycle_path"][0], "a.py");
        assert_eq!(v["structured"]["cycle_path"][2], "a.py");
    }
}
