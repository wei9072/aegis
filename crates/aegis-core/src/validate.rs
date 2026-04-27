//! `validate_change` — pure-library entry point.
//!
//! Given a proposed file write, runs Ring 0 syntax + Ring 0.5
//! signal extraction + cost-aware regression detection. Returns
//! a structured verdict (decision + reasons + signals).
//!
//! Same logic that `aegis-mcp` exposes over JSON-RPC and that the
//! Claude Code PreToolUse hook calls via `aegis check`. Lifting
//! it into a library lets aegis-agent's `LocalAegisPredictor` call
//! it in-process — no MCP subprocess needed.
//!
//! Negative-space contract preserved: this function only emits a
//! verdict. It never modifies disk, never proposes a fix, never
//! retries. Callers who get `BLOCK` MUST surface the reasons to the
//! agent / human; aegis itself never coaches.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::ast::registry::LanguageRegistry;
use crate::enforcement::check_syntax_native;
use crate::signal_layer_pyapi::extract_signals_native;

/// Top-level verdict shape. Stable wire format — `aegis-mcp` and
/// the upcoming `LocalAegisPredictor` both serve this exact shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateVerdict {
    /// `"PASS"`, `"WARN"`, or `"BLOCK"`.
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
        return ValidateVerdict {
            decision: "BLOCK".into(),
            reasons: vec![json!({
                "layer": "ring0",
                "decision": "block",
                "reason": "unsupported_extension",
                "detail": format!(
                    "no language adapter for {suffix:?}; supported: {:?}",
                    supported_exts
                ),
            })],
            signals_after: Map::new(),
            signals_before: None,
            regression_detail: None,
        };
    }

    let tmp_new = match write_temp(&suffix, new_content) {
        Ok(p) => p,
        Err(e) => {
            return ValidateVerdict {
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
            };
        }
    };

    if let Ok(violations) = check_syntax_native(&tmp_new) {
        for v in violations {
            reasons.push(json!({
                "layer": "ring0",
                "decision": "block",
                "reason": "ring0_violation",
                "detail": v,
            }));
        }
    }

    let new_sigs = match extract_signals_native(&tmp_new) {
        Ok(v) => v,
        Err(e) => {
            cleanup(&tmp_new);
            return ValidateVerdict {
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

    if let Some(old) = old_content {
        if let Ok(old_path) = write_temp(&suffix, old) {
            let old_sigs = extract_signals_native(&old_path).unwrap_or_default();
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

            let cost_after: f64 = new_sigs.iter().map(|s| s.value).sum();
            let cost_before: f64 = old_sigs.iter().map(|s| s.value).sum();
            if cost_after > cost_before {
                let mut growers: Map<String, Value> = Map::new();
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
                        growers.insert(key, json!(delta));
                    }
                }
                regression_detail = Some(growers.clone());
                reasons.push(json!({
                    "layer": "regression",
                    "decision": "block",
                    "reason": "cost_increased",
                    "detail": format!(
                        "total cost {cost_before:.0} → {cost_after:.0}; growers: {:?}",
                        growers
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

    ValidateVerdict {
        decision: decision.into(),
        reasons,
        signals_after,
        signals_before,
        regression_detail,
    }
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
    fn block_on_unsupported_extension() {
        let v = validate_change("notes.xyz", "anything", None);
        assert_eq!(v.decision, "BLOCK");
        assert!(v.reasons[0]["reason"]
            .as_str()
            .unwrap()
            .contains("unsupported_extension"));
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
    fn no_regression_when_cost_unchanged() {
        let body = "x = 1\n";
        let v = validate_change("same.py", body, Some(body));
        assert_eq!(v.decision, "PASS");
        assert!(v.regression_detail.is_none());
        // signals_before populated when old supplied.
        assert!(v.signals_before.is_some());
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
