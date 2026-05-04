//! `validate_change` — pure-library entry point.
//!
//! Given a proposed file write, runs Ring 0 syntax + Ring 0.5
//! signal extraction + cost-aware regression detection. Returns
//! a structured verdict (decision + reasons + signals).
//!
//! Same logic that `aegis-mcp` exposes over JSON-RPC and that the
//! Claude Code PreToolUse hook calls via `aegis check`. Lifting
//! it into a library lets in-process callers invoke the gate
//! directly — no MCP subprocess needed.
//!
//! Negative-space contract preserved: this function only emits a
//! verdict. It never modifies disk, never proposes a fix, never
//! retries. Callers who get `BLOCK` MUST surface the reasons to the
//! agent / human; aegis itself never coaches.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::ast::registry::LanguageRegistry;
use crate::enforcement::check_syntax_native_detailed;
use crate::security::check_security;
use crate::signal_layer_pyapi::{extract_signals_native, SignalData};
use crate::signals::unresolved_local_import_count;
use crate::workspace::{public_symbols_lost, summarize_file, WorkspaceIndex};

/// Wire-format schema version. Bump when the verdict shape changes
/// in a non-additive way so MCP clients can branch on it.
pub const VERDICT_SCHEMA_VERSION: &str = "2";

/// Top-level verdict shape. Stable wire format — `aegis-mcp` and
/// the upcoming `LocalAegisPredictor` both serve this exact shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateVerdict {
    /// Schema version. Bump on non-additive changes.
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    /// `"PASS"`, `"WARN"`, `"BLOCK"`, or `"SKIP"` (file type unknown
    /// to aegis — tells the agent "I have no opinion" instead of
    /// blocking on `.md` / `.toml` / `.json`).
    pub decision: String,
    /// Each gate's per-violation breakdown. Empty on PASS.
    #[serde(default)]
    pub reasons: Vec<Value>,
    /// Sum of signal values per name for the proposed `new_content`.
    /// Defaults to empty map so older / partial wire payloads (e.g.
    /// a ring0 syntax-error verdict where signal extraction never
    /// ran) deserialize without losing the decision.
    #[serde(default)]
    pub signals_after: Map<String, Value>,
    /// Same shape as `signals_after` but for `old_content`. Only
    /// populated when `old_content` is supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signals_before: Option<Map<String, Value>>,
    /// Per-signal positive delta (only signals that grew). Only
    /// populated when cost regressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regression_detail: Option<Map<String, Value>>,
    /// Per-signal verdict: "improved" | "unchanged" | "regressed".
    /// Only populated when `old_content` was supplied. Lets agents
    /// see the full delta picture (not just regressions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_deltas: Option<Map<String, Value>>,
    /// S2.2: when multiple layers fired, this names the single
    /// "fix this first" reason for the agent. Selected by priority:
    /// Ring 0 syntax > Ring R2 cycle > Ring 0.7 security > Ring R2
    /// public_symbol_removed > Ring 0.5 signal regression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_blocker: Option<Value>,
}

fn default_schema_version() -> String {
    VERDICT_SCHEMA_VERSION.to_string()
}

/// Pick the single most-actionable reason from a list of `reason`
/// JSON values. Priority order is fixed (S2.2):
///   1. Ring 0 syntax errors
///   2. Ring R2 cycle introductions
///   3. Ring 0.7 security violations
///   4. Ring R2 public symbol removals
///   5. Ring 0.5 cost regressions
///   6. Anything else
/// Returns None when reasons is empty or contains only non-blocking
/// entries.
fn primary_blocker(reasons: &[Value]) -> Option<Value> {
    fn priority(r: &Value) -> u32 {
        let layer = r.get("layer").and_then(|v| v.as_str()).unwrap_or("");
        let reason = r.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        match (layer, reason) {
            ("ring0", _) => 1,
            ("ringR2", "cycle_introduced") => 2,
            ("ring0_7", _) => 3,
            ("ringR2", "public_symbol_removed") => 4,
            ("regression", _) => 5,
            _ => 100,
        }
    }
    let blocker = reasons
        .iter()
        .filter(|r| {
            r.get("decision").and_then(|d| d.as_str()) == Some("block")
        })
        .min_by_key(|r| priority(r))?;
    Some(blocker.clone())
}

impl ValidateVerdict {
    /// Convenience: was the verdict BLOCK?
    #[must_use]
    pub fn blocked(&self) -> bool {
        self.decision == "BLOCK"
    }

    /// Re-emit as a `serde_json::Value` matching the V0.x +
    /// `aegis-mcp` wire format that downstream consumers expect.
    #[must_use]
    pub fn to_value(&self) -> Value {
        // Round-trip via serde so the field-presence rules
        // (`signals_before` / `regression_detail` omitted when None)
        // stay in one place.
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Run the full validate-change pipeline:
///   - Ring 0 syntax check on the proposed `new_content`
///   - Ring 0.5 signal extraction
///   - If `old_content` provided: extract signals on the old too,
///     compare cost totals, emit `regression` reason on growth
///   - Aggregate reasons → decision (BLOCK > WARN > PASS)
///
/// `path` is used only for its file extension (selects the language
/// adapter); no IO happens against `path` itself. Both `new_content`
/// and `old_content` are written to temp files for the existing
/// file-based aegis-core APIs, then cleaned up.
pub fn validate_change(
    path: &str,
    new_content: &str,
    old_content: Option<&str>,
) -> ValidateVerdict {
    let mut reasons: Vec<Value> = Vec::new();

    let suffix = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_else(|| ".py".to_string());

    let supported_exts = LanguageRegistry::global().extensions();
    if !supported_exts.contains(&suffix.as_str()) {
        // SKIP, not BLOCK. aegis has no opinion on .md / .toml /
        // .json / .yaml etc. — telling the upstream agent BLOCK on
        // a markdown edit makes it think its markdown is wrong.
        return ValidateVerdict {
            schema_version: VERDICT_SCHEMA_VERSION.into(),
            decision: "SKIP".into(),
            reasons: vec![json!({
                "layer": "ring0",
                "decision": "skip",
                "reason": "unsupported_extension",
                "detail": format!(
                    "no language adapter for {suffix:?}; aegis has no opinion on this file type"
                ),
            })],
            signals_after: Map::new(),
            signals_before: None,
            regression_detail: None,
            signal_deltas: None,
            primary_blocker: None,
        };
    }

    let tmp_new = match write_temp(&suffix, new_content) {
        Ok(p) => p,
        Err(e) => {
            return ValidateVerdict {
                schema_version: VERDICT_SCHEMA_VERSION.into(),
                decision: "BLOCK".into(),
                reasons: vec![json!({
                    "layer": "ring0",
                    "decision": "block",
                    "reason": "tempfile_error",
                    "detail": e,
                })],
                signals_after: Map::new(),
                signals_before: None,
                regression_detail: None,
                signal_deltas: None,
                primary_blocker: None,
            };
        }
    };

    let mut ring0_failed = false;
    if let Ok(violations) = check_syntax_native_detailed(&tmp_new) {
        if !violations.is_empty() {
            ring0_failed = true;
        }
        for v in violations {
            reasons.push(json!({
                "layer": "ring0",
                "decision": "block",
                "reason": "ring0_violation",
                "detail": v.message,
                "range": {
                    "start_line": v.start_line,
                    "start_col": v.start_col,
                    "end_line": v.end_line,
                    "end_col": v.end_col,
                },
                "node_kind": v.kind,
            }));
        }
    }

    // S2.2: layer short-circuit. When Ring 0 (syntax) fails, every
    // downstream layer (security pattern matching, signal extraction,
    // workspace analysis) is operating on a degenerate AST and will
    // emit noise. Return immediately with just the Ring 0 reasons so
    // the upstream agent gets a clean "fix syntax first" signal.
    if ring0_failed {
        cleanup(&tmp_new);
        let primary = primary_blocker(&reasons);
        return ValidateVerdict {
            schema_version: VERDICT_SCHEMA_VERSION.into(),
            decision: "BLOCK".into(),
            reasons,
            signals_after: Map::new(),
            signals_before: None,
            regression_detail: None,
            signal_deltas: None,
            primary_blocker: primary,
        };
    }

    // Ring 0.7 — security violations (boolean, not delta-based).
    // Distinct from cost regression: these are absolute BLOCKs
    // because the patterns have no legitimate use we couldn't
    // refactor. `aegis-allow: <rule-id>` line comments opt out.
    for sv in check_security(path, new_content) {
        reasons.push(crate::reasons::ring0_7_security(
            &sv.rule_id,
            &sv.severity,
            sv.message,
            crate::reasons::Range {
                start_line: sv.start_line,
                start_col: sv.start_col,
                end_line: sv.end_line,
                end_col: sv.end_col,
            },
        ));
    }

    let new_sigs = match extract_signals_native(&tmp_new) {
        Ok(mut v) => {
            // Workspace-sensitive: the AST-only extract_signals_native
            // can't see the parent directory of the *real* file, so
            // we run unresolved_local_import_count here against the
            // claimed `path`. If `path` doesn't have a real parent
            // on disk (e.g. unit-test fixtures), this is a no-op.
            v.push(SignalData {
                name: "unresolved_local_import_count".into(),
                value: unresolved_local_import_count(path, new_content),
                file_path: path.to_string(),
                description: "Relative imports that don't resolve to an existing file".into(),
            });
            v
        }
        Err(e) => {
            cleanup(&tmp_new);
            return ValidateVerdict {
                schema_version: VERDICT_SCHEMA_VERSION.into(),
                decision: "BLOCK".into(),
                reasons: vec![json!({
                    "layer": "ring0_5",
                    "decision": "block",
                    "reason": "signal_extraction_failed",
                    "detail": e,
                })],
                signals_after: Map::new(),
                signals_before: None,
                regression_detail: None,
                signal_deltas: None,
                primary_blocker: None,
            };
        }
    };

    let mut signals_after: Map<String, Value> = Map::new();
    for s in &new_sigs {
        signals_after
            .entry(s.name.clone())
            .and_modify(|v| {
                let cur = v.as_f64().unwrap_or(0.0);
                *v = json!(cur + s.value);
            })
            .or_insert(json!(s.value));
    }

    let mut signals_before: Option<Map<String, Value>> = None;
    let mut regression_detail: Option<Map<String, Value>> = None;
    let mut signal_deltas: Option<Map<String, Value>> = None;

    if let Some(old) = old_content {
        if let Ok(old_path) = write_temp(&suffix, old) {
            let mut old_sigs = extract_signals_native(&old_path).unwrap_or_default();
            old_sigs.push(SignalData {
                name: "unresolved_local_import_count".into(),
                value: unresolved_local_import_count(path, old),
                file_path: path.to_string(),
                description: "Relative imports that don't resolve to an existing file".into(),
            });
            cleanup(&old_path);

            let mut sb: Map<String, Value> = Map::new();
            for s in &old_sigs {
                sb.entry(s.name.clone())
                    .and_modify(|v| {
                        let cur = v.as_f64().unwrap_or(0.0);
                        *v = json!(cur + s.value);
                    })
                    .or_insert(json!(s.value));
            }
            signals_before = Some(sb.clone());

            // Per-signal regression: any single signal getting worse
            // triggers BLOCK. The previous sum-based check let one
            // improving signal silently mask another that regressed
            // (e.g. fan_out 5→3 with chain_depth 2→4 summed to 7=7).
            let mut growers: Map<String, Value> = Map::new();
            let mut shrinkers: Map<String, Value> = Map::new();
            let mut deltas: Map<String, Value> = Map::new();
            let keys: std::collections::BTreeSet<String> = signals_after
                .keys()
                .chain(sb.keys())
                .cloned()
                .collect();
            for key in keys {
                let a = signals_after.get(&key).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let b = sb.get(&key).and_then(|v| v.as_f64()).unwrap_or(0.0);
                if a > b {
                    let delta = ((a - b) * 10_000.0).round() / 10_000.0;
                    growers.insert(key.clone(), json!(delta));
                    deltas.insert(key, json!({"status": "regressed", "delta": delta}));
                } else if b > a {
                    let delta = ((b - a) * 10_000.0).round() / 10_000.0;
                    shrinkers.insert(key.clone(), json!(delta));
                    deltas.insert(key, json!({"status": "improved", "delta": delta}));
                } else {
                    deltas.insert(key, json!({"status": "unchanged", "delta": 0.0}));
                }
            }
            signal_deltas = Some(deltas);
            if !growers.is_empty() {
                regression_detail = Some(growers.clone());
                reasons.push(json!({
                    "layer": "regression",
                    "decision": "block",
                    "reason": "signal_regressed",
                    "detail": format!(
                        "regressed signals: {:?}; improved: {:?}",
                        growers, shrinkers
                    ),
                }));
            }
        }
    }

    cleanup(&tmp_new);

    let any_block = reasons
        .iter()
        .any(|r| r.get("decision").and_then(|d| d.as_str()) == Some("block"));
    let any_warn = reasons
        .iter()
        .any(|r| r.get("decision").and_then(|d| d.as_str()) == Some("warn"));
    let decision = if any_block {
        "BLOCK"
    } else if any_warn {
        "WARN"
    } else {
        "PASS"
    };

    let primary = primary_blocker(&reasons);
    ValidateVerdict {
        schema_version: VERDICT_SCHEMA_VERSION.into(),
        decision: decision.into(),
        reasons,
        signals_after,
        signals_before,
        regression_detail,
        signal_deltas,
        primary_blocker: primary,
    }
}

/// Ring R2 — workspace-aware validation. Runs the full single-file
/// `validate_change` first, then layers cross-file structural checks
/// on top:
///   - cycle detection (would the change introduce a module cycle?)
///   - public symbol loss (did the change delete public symbols?)
///   - workspace-wide unresolved import delta (did the change break
///     any callers' imports?)
///
/// `workspace_root` should be the project root (containing the file
/// being changed). Set this to `None` to skip Ring R2 entirely —
/// equivalent to calling `validate_change`.
pub fn validate_change_with_workspace(
    path: &str,
    new_content: &str,
    old_content: Option<&str>,
    workspace_root: &str,
) -> ValidateVerdict {
    let mut verdict = validate_change(path, new_content, old_content);
    if verdict.decision == "SKIP" {
        return verdict;
    }
    // S2.2 layer-priority: if Ring 0/0.5/0.7 already blocked, skip
    // the workspace pass — its results would be unreliable on a
    // syntax-broken file anyway.
    if verdict.decision == "BLOCK"
        && verdict
            .primary_blocker
            .as_ref()
            .and_then(|p| p.get("ring").and_then(|r| r.as_str()))
            == Some("ring0")
    {
        return verdict;
    }

    let root = std::path::Path::new(workspace_root);
    if !root.is_dir() {
        return verdict;
    }
    let path_buf = std::path::PathBuf::from(path);
    // S5: use the cached build so repeated PreToolUse hook calls
    // re-parse only changed files, not the whole tree every time.
    let baseline = WorkspaceIndex::build_cached(root);

    // --- Cycle introduction ---
    let after = baseline.with_change(&path_buf, new_content);
    let after_cycle = after.find_cycle();
    if baseline.find_cycle().is_empty() && !after_cycle.is_empty() {
        verdict
            .reasons
            .push(crate::reasons::ringR2_cycle_introduced(after_cycle, path));
        verdict.decision = "BLOCK".into();
    }

    // --- Public-symbol loss ---
    let new_summary = summarize_file(&path_buf, new_content);
    let old_summary = if let Some(old) = old_content {
        summarize_file(&path_buf, old)
    } else {
        baseline.files.get(&path_buf).cloned().unwrap_or_default()
    };
    let lost = public_symbols_lost(&old_summary, &new_summary);
    if !lost.is_empty() {
        // Only block if any other file still references the lost
        // symbols by name (e.g., `from .api import helper`). Pure
        // private cleanup (no callers) is fine.
        // S3.2: build a structured map { symbol -> [caller_paths] }
        // so the agent can see exactly which files break.
        let mut callers_map: serde_json::Map<String, Value> = serde_json::Map::new();
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
            verdict
                .reasons
                .push(crate::reasons::ringR2_public_symbol_removed(
                    &still_referenced,
                    callers_map,
                ));
            verdict.decision = "BLOCK".into();
        }
    }

    verdict
}

fn write_temp(suffix: &str, content: &str) -> Result<String, String> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("aegis-validate-{pid}-{ts}{suffix}"));
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_on_clean_python() {
        let v = validate_change("trivial.py", "x = 1\n", None);
        assert_eq!(v.decision, "PASS");
        assert!(v.reasons.is_empty());
    }

    #[test]
    fn block_on_python_syntax_error() {
        let v = validate_change("broken.py", "def f(\n", None);
        assert_eq!(v.decision, "BLOCK");
        assert!(v.reasons.iter().any(|r| r["layer"] == "ring0"));
    }

    #[test]
    fn pass_on_clean_rust() {
        let v = validate_change(
            "lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
            None,
        );
        assert_eq!(v.decision, "PASS", "expected clean Rust to PASS; got {v:?}");
    }

    #[test]
    fn block_on_rust_syntax_error() {
        let v = validate_change("broken.rs", "fn broken(\n", None);
        assert_eq!(v.decision, "BLOCK");
        assert!(v.reasons.iter().any(|r| r["layer"] == "ring0"));
    }

    #[test]
    fn skip_on_unsupported_extension() {
        // P1.6: BLOCK was a usability bug — agents editing .md/.toml
        // would think their markdown was wrong. Now: SKIP with a
        // "no opinion" reason so the agent moves on.
        let v = validate_change("notes.xyz", "anything", None);
        assert_eq!(v.decision, "SKIP");
        assert!(v.reasons[0]["reason"]
            .as_str()
            .unwrap()
            .contains("unsupported_extension"));
    }

    #[test]
    fn schema_version_stamped() {
        let v = validate_change("trivial.py", "x = 1\n", None);
        assert_eq!(v.schema_version, VERDICT_SCHEMA_VERSION);
    }

    #[test]
    fn ring0_failure_short_circuits_other_layers() {
        // S2.2: when syntax fails, downstream layers (Ring 0.7, 0.5)
        // operate on a bad parse and would emit noise. We early-return
        // so the agent gets only the actionable Ring 0 reasons.
        let v = validate_change("broken.py", "def f(\n", None);
        assert_eq!(v.decision, "BLOCK");
        // No ring0_7 / regression / ring0_5 entries when ring0 fails.
        assert!(
            v.reasons.iter().all(|r| r["layer"] == "ring0"),
            "expected only ring0 reasons after short-circuit; got {v:?}"
        );
    }

    #[test]
    fn primary_blocker_picks_highest_priority() {
        // S2.2: when multiple layers fire, primary_blocker names the
        // single thing the agent should fix first.
        let v = validate_change("broken.py", "def f(\n", None);
        let primary = v.primary_blocker.expect("primary_blocker required on BLOCK");
        assert_eq!(primary["layer"], "ring0");
    }

    #[test]
    fn ring0_violation_carries_line_range() {
        let v = validate_change("broken.py", "def f(\n", None);
        assert_eq!(v.decision, "BLOCK");
        let r = v.reasons.iter().find(|r| r["layer"] == "ring0").unwrap();
        assert!(r.get("range").is_some(), "ring0 violation must carry range");
        let rng = r.get("range").unwrap();
        assert!(rng["start_line"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn signal_deltas_populated_when_old_supplied() {
        let body = "x = 1\n";
        let v = validate_change("same.py", body, Some(body));
        let deltas = v.signal_deltas.expect("signal_deltas should be set");
        assert!(!deltas.is_empty());
    }

    #[test]
    fn block_on_cost_regression() {
        // Old: simple file with 1 import. New: same file with many
        // imports → fan_out grows → cost regresses.
        let old = "import os\n";
        let new = "import os\nimport sys\nimport json\nimport time\nimport random\n\
                   import math\nimport re\nimport itertools\nimport functools\n\
                   import collections\nimport pathlib\nimport hashlib\n";
        let v = validate_change("foo.py", new, Some(old));
        assert_eq!(v.decision, "BLOCK", "expected regression to BLOCK; got {v:?}");
        assert!(v.regression_detail.is_some());
    }

    #[test]
    fn block_when_one_signal_regresses_even_if_total_unchanged() {
        // Regression: previously a sum-only check let one signal mask another.
        // Old: many shallow imports (high fan_out, low chain_depth).
        // New: few imports but a deep call chain — total may be similar
        //   or even lower, but chain_depth got worse → still BLOCK.
        let old = "import os\nimport sys\nimport json\nimport re\nx = 1\n";
        let new = "import os\nresult = a.b.c.d.e.f.g.h.i.j.k.l.m.n.o\n";
        let v = validate_change("mix.py", new, Some(old));
        assert_eq!(
            v.decision, "BLOCK",
            "per-signal regression should BLOCK even when sum drops; got {v:?}"
        );
        let detail = v.regression_detail.expect("regression_detail must be set");
        assert!(
            detail.contains_key("max_chain_depth"),
            "expected max_chain_depth in growers; got {detail:?}"
        );
    }

    #[test]
    fn no_regression_when_cost_unchanged() {
        let body = "x = 1\n";
        let v = validate_change("same.py", body, Some(body));
        assert_eq!(v.decision, "PASS");
        assert!(v.regression_detail.is_none());
        // signals_before populated when old supplied.
        assert!(v.signals_before.is_some());
    }

    #[test]
    fn ring_r2_blocks_when_change_introduces_cycle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "x = 1\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "from .a import x\n").unwrap();
        let v = validate_change_with_workspace(
            dir.path().join("a.py").to_str().unwrap(),
            "from .b import y\n",
            Some("x = 1\n"),
            dir.path().to_str().unwrap(),
        );
        assert_eq!(v.decision, "BLOCK", "expected cycle BLOCK, got {v:?}");
        assert!(v.reasons.iter().any(|r| r["reason"] == "cycle_introduced"));
    }

    #[test]
    fn ring_r2_blocks_when_removing_referenced_public_symbol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("api.py"), "def helper(): pass\ndef other(): pass\n").unwrap();
        std::fs::write(dir.path().join("caller.py"), "from .api import helper\n").unwrap();
        let v = validate_change_with_workspace(
            dir.path().join("api.py").to_str().unwrap(),
            "def other(): pass\n",
            Some("def helper(): pass\ndef other(): pass\n"),
            dir.path().to_str().unwrap(),
        );
        assert_eq!(v.decision, "BLOCK", "expected pub-symbol BLOCK, got {v:?}");
        assert!(v.reasons.iter().any(|r| r["reason"] == "public_symbol_removed"));
    }

    #[test]
    fn ring_r2_allows_unreferenced_pub_symbol_removal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("api.py"), "def unused(): pass\ndef other(): pass\n").unwrap();
        let v = validate_change_with_workspace(
            dir.path().join("api.py").to_str().unwrap(),
            "def other(): pass\n",
            Some("def unused(): pass\ndef other(): pass\n"),
            dir.path().to_str().unwrap(),
        );
        // Pure cleanup — no callers, so removal is fine.
        assert_ne!(v.decision, "BLOCK", "expected pass for unreferenced removal, got {v:?}");
    }

    #[test]
    fn to_value_roundtrip_matches_legacy_wire_shape() {
        let v = validate_change("trivial.py", "x = 1\n", None);
        let value = v.to_value();
        assert_eq!(value["decision"], "PASS");
        assert!(value.get("reasons").unwrap().is_array());
        assert!(value.get("signals_after").unwrap().is_object());
        // Optional fields omitted when None.
        assert!(value.get("signals_before").is_none());
        assert!(value.get("regression_detail").is_none());
    }
}
