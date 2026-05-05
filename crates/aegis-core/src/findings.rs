//! V2 Finding wire format — pure facts, no judgment.
//!
//! The V1 `ValidateVerdict` couples observation with judgment via
//! `decision: BLOCK/WARN/PASS/SKIP`, severity-tagged signals, and
//! `aegis-allow` filter-then-suppress semantics. V2 separates them:
//! aegis-core only emits *findings*; the consuming agent (LLM) decides
//! what to do with each.
//!
//! Mapping between V1 reasons and V2 findings:
//! - Ring 0 violation     → `Finding { kind: Syntax,    rule_id: "ring0_violation" }`
//! - Ring 0.5 signal grew → `Finding { kind: Signal,    rule_id: <signal_name> }`
//!   plus before/after value in `context["value_before"|"value_after"|"delta"]`
//! - Ring 0.7 SEC rule    → `Finding { kind: Security,  rule_id: "SEC00X" }`
//!   `aegis-allow: SEC00X` near the line still parses but now sets
//!   `user_acknowledged: true` instead of dropping the finding.
//! - Ring R2 cycle        → `Finding { kind: Workspace, rule_id: "cycle_introduced" }`
//! - Ring R2 sym removal  → `Finding { kind: Workspace, rule_id: "public_symbol_removed" }`
//! - File role / z-score  → `Finding { kind: Workspace, rule_id: "file_role" }`

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::ast::parsed_file::{parse as parse_file, ParsedFile};
use crate::ast::registry::LanguageRegistry;
use crate::enforcement::syntax_violations;
use crate::security::check_security;
use crate::signal_extraction::extract_signals;
use crate::signals::unresolved_local_import_count;
use crate::workspace::{public_symbols_lost, summarize_file, WorkspaceIndex};

/// Wire-format schema version for the V2 findings shape. Independent
/// of the V1 `VERDICT_SCHEMA_VERSION` so consumers can branch on
/// which output style they expect.
pub const FINDINGS_SCHEMA_VERSION: &str = "v2.0";

/// What kind of finding this is. Used by consumers to route findings
/// (e.g., "show all syntax errors first" or "ignore workspace
/// findings on this PR"). No severity — that's intentional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Tree-sitter ERROR or MISSING node. Single-file, structural.
    Syntax,
    /// Ring 0.5 structural smell (fan_out, empty_handler_count, …).
    /// `context.delta` is set when both old & new were observed.
    Signal,
    /// Ring 0.7 security pattern (SEC001–SEC010).
    Security,
    /// Ring R2 cross-file finding (cycle, public-symbol-removed,
    /// file-role / z-score outlier).
    Workspace,
}

impl FindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::Syntax => "syntax",
            FindingKind::Signal => "signal",
            FindingKind::Security => "security",
            FindingKind::Workspace => "workspace",
        }
    }
}

/// Source-code range, 1-based line/col (mirrors the `reasons::Range`
/// shape from V1 so consumers don't need a separate parser).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// A single observation. Pure fact — no severity, no decision, no
/// suggestion. The consuming LLM decides whether to act on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// Stable identifier for the rule that produced this finding.
    /// Examples: `"ring0_violation"`, `"empty_handler_count"`,
    /// `"SEC009"`, `"cycle_introduced"`, `"public_symbol_removed"`,
    /// `"file_role"`.
    pub rule_id: String,
    /// File this finding is about (the *target* file, not necessarily
    /// where the issue manifests — e.g. for a public_symbol_removed
    /// finding, this is the file that lost the symbol).
    pub file: PathBuf,
    /// Optional range — present for syntax / security / per-line
    /// signal findings. Absent for whole-file aggregates and most
    /// workspace findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    /// Optional source snippet at `range`. Lets LLM see context
    /// without re-reading the file. Capped to a few lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Rule-specific structured payload. For signals: `value_before`,
    /// `value_after`, `delta`. For security: `severity_hint` (one of
    /// "block"/"warn", carried as a *hint* to the LLM, not a verdict).
    /// For workspace: `cycle: [paths]`, `callers: { sym: [paths] }`,
    /// `role`, `fan_in`, `fan_out`, `z_score`, etc.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub context: Map<String, Value>,
    /// True iff the user opted out via `aegis-allow: <rule_id>` (or
    /// `aegis-allow: all`) on the same or previous source line. The
    /// finding is still emitted; consumers decide what to do with the
    /// acknowledgement. PR 4 changes the V1 filter-and-drop semantics
    /// to "keep but flag".
    #[serde(default)]
    pub user_acknowledged: bool,
}

impl Finding {
    pub fn new(kind: FindingKind, rule_id: impl Into<String>, file: PathBuf) -> Self {
        Finding {
            kind,
            rule_id: rule_id.into(),
            file,
            range: None,
            snippet: None,
            context: Map::new(),
            user_acknowledged: false,
        }
    }

    pub fn with_range(mut self, range: Range) -> Self {
        self.range = Some(range);
        self
    }

    pub fn with_context(mut self, key: &str, value: Value) -> Self {
        self.context.insert(key.into(), value);
        self
    }

    pub fn with_snippet(mut self, snippet: String) -> Self {
        self.snippet = Some(snippet);
        self
    }

    pub fn acknowledged(mut self) -> Self {
        self.user_acknowledged = true;
        self
    }
}

/// V2 entry point — emit all findings for a proposed file write. No
/// decision, no judgment. Caller (the LLM) decides what to do with
/// each finding.
///
/// `old_content` enables delta context on Signal findings. When
/// omitted, signals are reported as absolute counts; when supplied,
/// `context.value_before`, `value_after`, `delta` are populated.
pub fn gather_findings(
    path: &str,
    new_content: &str,
    old_content: Option<&str>,
) -> Vec<Finding> {
    // Unsupported extension → no opinion → empty findings list.
    let supported_exts = LanguageRegistry::global().extensions();
    let suffix = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    if !supported_exts.contains(&suffix.as_str()) {
        return vec![];
    }

    let Some(parsed_new) = parse_file(path, new_content) else {
        return vec![];
    };
    let parsed_old = old_content.and_then(|c| parse_file(path, c));

    let mut findings = Vec::new();
    let file_path = PathBuf::from(path);

    // Syntax findings — tree-sitter ERROR/MISSING anchors.
    for v in syntax_violations(&parsed_new, path) {
        let mut f = Finding::new(FindingKind::Syntax, "syntax_error", file_path.clone())
            .with_range(Range {
                start_line: v.start_line,
                start_col: v.start_col,
                end_line: v.end_line,
                end_col: v.end_col,
            })
            .with_context("node_kind", json!(v.kind))
            .with_context("message", json!(v.message));
        f = annotate_acknowledgement(f, "syntax_error", new_content);
        findings.push(f);
    }

    // Security findings — emit every match; LLM decides which matter.
    // V2 doesn't short-circuit on syntax errors; LLM can choose to
    // ignore security findings on a syntactically broken file.
    for sv in check_security(&parsed_new) {
        let mut f = Finding::new(FindingKind::Security, &sv.rule_id, file_path.clone())
            .with_range(Range {
                start_line: sv.start_line,
                start_col: sv.start_col,
                end_line: sv.end_line,
                end_col: sv.end_col,
            })
            .with_context("severity_hint", json!(sv.severity))
            .with_context("message", json!(sv.message));
        // V2: aegis-allow comments annotate, do not filter. Each
        // security match is checked individually because a single
        // file may have several SEC findings, only some of which the
        // user has acknowledged.
        let rule_id = sv.rule_id.clone();
        f = annotate_acknowledgement(f, &rule_id, new_content);
        findings.push(f);
    }

    // Signals (Ring 0.5) — emit value & delta as context.
    let new_sigs = extract_signals(&parsed_new, path);
    let old_sigs = parsed_old
        .as_ref()
        .map(|p| extract_signals(p, path));
    let new_unresolved =
        unresolved_local_import_count(&parsed_new, path);
    let old_unresolved = parsed_old
        .as_ref()
        .map(|p| unresolved_local_import_count(p, path));

    let mut new_values: std::collections::BTreeMap<String, f64> =
        new_sigs.iter().map(|s| (s.name.clone(), s.value)).collect();
    new_values.insert("unresolved_local_import_count".into(), new_unresolved);

    let old_values: Option<std::collections::BTreeMap<String, f64>> =
        old_sigs.as_ref().map(|sigs| {
            let mut m: std::collections::BTreeMap<String, f64> =
                sigs.iter().map(|s| (s.name.clone(), s.value)).collect();
            if let Some(v) = old_unresolved {
                m.insert("unresolved_local_import_count".into(), v);
            }
            m
        });

    for (name, value_after) in &new_values {
        let mut f = Finding::new(FindingKind::Signal, name, file_path.clone())
            .with_context("value_after", json!(value_after));
        if let Some(old_map) = old_values.as_ref() {
            let value_before = old_map.get(name).copied().unwrap_or(0.0);
            let delta = value_after - value_before;
            f = f
                .with_context("value_before", json!(value_before))
                .with_context("delta", json!(delta));
        }
        findings.push(f);
    }

    findings
}

/// V2 workspace-aware variant — adds Ring R2 findings (cycle,
/// public_symbol_removed, file_role) on top of `gather_findings`.
pub fn gather_findings_with_workspace(
    path: &str,
    new_content: &str,
    old_content: Option<&str>,
    workspace_root: &str,
) -> Vec<Finding> {
    let mut findings = gather_findings(path, new_content, old_content);

    let root = std::path::Path::new(workspace_root);
    if !root.is_dir() {
        return findings;
    }

    let path_buf = PathBuf::from(path);
    let baseline = WorkspaceIndex::build_cached(root);

    let parsed_new_for_ws = parse_file(path, new_content);
    let after = match parsed_new_for_ws.as_ref() {
        Some(p) => baseline.with_change(&path_buf, p),
        None => baseline.clone(),
    };

    // file_role finding — pure context, no judgment about whether the
    // role is "good" or "bad". LLM decides if e.g. an entry-file with
    // high fan_out is expected (yes) or anomalous.
    let role = after.role_hint(&path_buf);
    let fan_in = after.fan_in(&path_buf);
    let fan_out_proj = after.fan_out(&path_buf);
    let instability = after.instability(&path_buf);
    let fan_out_z = after.fan_out_z_score(&path_buf);
    let fan_in_z = after.fan_in_z_score(&path_buf);
    let project_fan_out_stats = after.fan_out_stats();
    let mut role_finding = Finding::new(FindingKind::Workspace, "file_role", path_buf.clone())
        .with_context("role", json!(role))
        .with_context("fan_in", json!(fan_in))
        .with_context("fan_out", json!(fan_out_proj))
        .with_context("instability", json!(instability));
    if let Some(z) = fan_out_z {
        role_finding = role_finding.with_context("fan_out_z_score", json!(z));
    }
    if let Some(z) = fan_in_z {
        role_finding = role_finding.with_context("fan_in_z_score", json!(z));
    }
    if let Some((m, s, _)) = project_fan_out_stats {
        role_finding = role_finding
            .with_context("project_fan_out_median", json!(m))
            .with_context("project_fan_out_std", json!(s));
    }
    findings.push(role_finding);

    // Cycle introduction
    let after_cycle = after.find_cycle();
    if baseline.find_cycle().is_empty() && !after_cycle.is_empty() {
        findings.push(
            Finding::new(FindingKind::Workspace, "cycle_introduced", path_buf.clone())
                .with_context("cycle", json!(after_cycle)),
        );
    }

    // Public-symbol loss
    let new_summary = match parsed_new_for_ws.as_ref() {
        Some(p) => summarize_file(&path_buf, p),
        None => return findings,
    };
    let old_summary = if let Some(old) = old_content {
        match parse_file(path, old) {
            Some(p) => summarize_file(&path_buf, &p),
            None => baseline.files.get(&path_buf).cloned().unwrap_or_default(),
        }
    } else {
        baseline.files.get(&path_buf).cloned().unwrap_or_default()
    };
    let lost = public_symbols_lost(&old_summary, &new_summary);
    if !lost.is_empty() {
        let mut callers_map: Map<String, Value> = Map::new();
        let mut still_referenced: Vec<String> = Vec::new();
        for sym in &lost {
            let mut callers: Vec<String> = Vec::new();
            for (p, s) in &baseline.files {
                if p.as_path() == path_buf.as_path() {
                    continue;
                }
                if s.imported_symbols.contains(sym.as_str()) {
                    callers.push(p.to_string_lossy().into_owned());
                }
            }
            if !callers.is_empty() {
                still_referenced.push(sym.clone());
                callers_map.insert(sym.clone(), json!(callers));
            }
        }
        if !still_referenced.is_empty() {
            findings.push(
                Finding::new(
                    FindingKind::Workspace,
                    "public_symbol_removed",
                    path_buf.clone(),
                )
                .with_context("symbols", json!(still_referenced))
                .with_context("callers", Value::Object(callers_map)),
            );
        }
    }

    findings
}

/// Mark a finding `user_acknowledged: true` when an `aegis-allow:
/// <rule_id>` (or `aegis-allow: all`) comment appears on the same or
/// previous line. Counterpart of V1's `suppress_allowed` filter, but
/// keeps the finding instead of dropping it.
fn annotate_acknowledgement(mut f: Finding, rule_id: &str, code: &str) -> Finding {
    let Some(range) = f.range.as_ref() else {
        return f;
    };
    let lines: Vec<&str> = code.lines().collect();
    let line_idx = range.start_line.saturating_sub(1);
    let needle_specific = format!("aegis-allow: {}", rule_id);
    let needle_all = "aegis-allow: all";
    let mut hit = false;
    if let Some(line) = lines.get(line_idx) {
        if line.contains(&needle_specific) || line.contains(needle_all) {
            hit = true;
        }
    }
    if !hit && line_idx > 0 {
        if let Some(prev) = lines.get(line_idx - 1) {
            if prev.contains(&needle_specific) || prev.contains(needle_all) {
                hit = true;
            }
        }
    }
    if hit {
        f.user_acknowledged = true;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_findings_for_clean_python() {
        let f = gather_findings("clean.py", "x = 1\n", None);
        // Signal findings always emit (even at 0); but no syntax/security.
        assert!(!f.iter().any(|fnd| fnd.kind == FindingKind::Syntax));
        assert!(!f.iter().any(|fnd| fnd.kind == FindingKind::Security));
        // 14 + unresolved_local_import_count = 15 signal findings.
        assert!(
            f.iter().filter(|fnd| fnd.kind == FindingKind::Signal).count() >= 14,
            "expected at least 14 signal findings; got {f:?}"
        );
    }

    #[test]
    fn syntax_finding_emitted_on_broken_python() {
        let f = gather_findings("broken.py", "def foo(\n", None);
        assert!(f.iter().any(|fnd| fnd.kind == FindingKind::Syntax
            && fnd.rule_id == "syntax_error"));
    }

    #[test]
    fn no_findings_for_unsupported_extension() {
        let f = gather_findings("notes.md", "# heading", None);
        assert!(f.is_empty(), "unsupported ext should yield empty; got {f:?}");
    }

    #[test]
    fn signal_finding_includes_delta_when_old_supplied() {
        let old = "x = 1\n";
        let new = "x = 1\n# TODO: revisit\n";
        let findings = gather_findings("foo.py", new, Some(old));
        let unfinished = findings
            .iter()
            .find(|f| f.kind == FindingKind::Signal && f.rule_id == "unfinished_marker_count")
            .expect("should have unfinished_marker_count finding");
        assert!(unfinished.context.get("value_before").is_some());
        assert!(unfinished.context.get("value_after").is_some());
        assert!(unfinished.context.get("delta").is_some());
    }

    #[test]
    fn aegis_allow_annotates_security_finding() {
        // V2 semantics: aegis-allow comments do NOT drop the finding;
        // they set user_acknowledged: true so the consuming agent
        // knows the user explicitly opted out of this rule on this
        // line.
        let code = "import requests\nrequests.get(url, verify=False)  # aegis-allow: SEC003\n";
        let f = gather_findings("foo.py", code, None);
        let sec003 = f
            .iter()
            .find(|fnd| fnd.kind == FindingKind::Security && fnd.rule_id == "SEC003")
            .expect("SEC003 must still appear");
        assert!(
            sec003.user_acknowledged,
            "aegis-allow on the same line should set user_acknowledged"
        );
    }

    #[test]
    fn security_finding_emitted_for_eval_with_dynamic_arg() {
        let code = "def f(user_input):\n    eval(user_input)\n";
        let f = gather_findings("a.py", code, None);
        assert!(f.iter().any(|fnd| fnd.kind == FindingKind::Security
            && fnd.rule_id == "SEC001"));
    }
}
