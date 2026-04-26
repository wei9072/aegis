//! Pipeline metric helpers — pure aggregations over per-iteration
//! signal observations + the canonical plan-content hash.
//!
//! Mirrors the `aegis/runtime/pipeline.py` private helpers
//! (`_kind_counts`, `_kind_value_totals`, `_total_cost`,
//! `_regressed`, `_regression_detail`, `_hash_plan`) one-for-one.
//! Pure functions; no IO, no LLM calls. The Python loop calls into
//! these via the `aegis._core` re-exports, which means the loop's
//! progress / regression logic is now Rust ground truth.
//!
//! `Signal` is intentionally NOT modelled here — these helpers take
//! the *minimal* shape the loop needs (kind name + numeric value),
//! letting the PyShim layer extract the relevant fields from
//! whatever Signal type Python carries (V0.x `aegis.core.bindings.Signal`
//! today; could be anything that exposes `.name` / `.value` later).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use aegis_ir::{plan_to_json, PatchPlan};

/// Number of *instances* of each signal kind across every file.
/// `(kind_name, count)` pairs in deterministic key order.
///
/// `names` is a flat iterator of every signal's `.name` field across
/// every file (the Python helper iterates `for sig in sig_list for
/// sig_list in signals.values()`, which is the same flattening).
pub fn kind_counts<'a, I>(names: I) -> BTreeMap<String, u64>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for name in names {
        *out.entry(name.to_string()).or_insert(0) += 1;
    }
    out
}

/// Sum each signal kind's `.value` across every file.
///
/// Two files with `fan_out=15` and `fan_out=8` produce a fan_out
/// total of 23. This is the metric scenario runners need to track —
/// instance counts alone (a file either carries fan_out or not)
/// cannot reflect "fan_out dropped from 15 to 2".
pub fn kind_value_totals<'a, I>(items: I) -> BTreeMap<String, f64>
where
    I: IntoIterator<Item = (&'a str, f64)>,
{
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    for (name, value) in items {
        *out.entry(name.to_string()).or_insert(0.0) += value;
    }
    out
}

/// Sum every signal value across every file. New files with all-zero
/// signals contribute 0 — by design, so a benign split doesn't look
/// like regression.
pub fn total_cost<I>(values: I) -> f64
where
    I: IntoIterator<Item = f64>,
{
    values.into_iter().sum()
}

/// Did the patch make the codebase worse?
///
/// Cost-based, not instance-count-based — see the docstring on the
/// Python `_regressed` for the full rationale (the instance-count
/// strategy false-positive'd legitimate refactors that produced new
/// files; cost-based comparison answers the actual question).
pub fn regressed(before_total: f64, after_total: f64) -> bool {
    after_total > before_total
}

/// Per-kind cost deltas, restricted to kinds whose cost actually
/// rose. Round to 4 decimals to match the Python output exactly.
///
/// Returns the LLM-facing version of "why was this rolled back".
/// Empty map means "no regression". Used to populate
/// `PlanContext.previous_regression_detail` so the next planner turn
/// can address the specific cost that grew.
pub fn regression_detail(
    before: &BTreeMap<String, f64>,
    after: &BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
    let mut detail: BTreeMap<String, f64> = BTreeMap::new();
    let mut keys: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    keys.extend(before.keys());
    keys.extend(after.keys());
    for key in keys {
        let b = before.get(key).copied().unwrap_or(0.0);
        let a = after.get(key).copied().unwrap_or(0.0);
        let delta = a - b;
        if delta > 0.0 {
            detail.insert(key.clone(), round4(delta));
        }
    }
    detail
}

/// Stable plan-content hash. SHA-256 over `plan_to_dict(plan)` minus
/// the two volatile fields (`iteration` and `parent_id`) that change
/// across re-runs but don't change the plan's *content*.
///
/// JSON keys are emitted in sorted order (mirrors Python
/// `json.dumps(..., sort_keys=True)`), so two runs of the same plan
/// hash identically regardless of map insertion order. Returns the
/// 64-char lowercase hex digest.
pub fn hash_plan(plan: &PatchPlan) -> String {
    let mut value = plan_to_json(plan);
    if let Value::Object(ref mut map) = value {
        map.remove("iteration");
        map.remove("parent_id");
    }
    let blob = canonical_json_bytes(&value);
    let mut hasher = Sha256::new();
    hasher.update(&blob);
    hex::encode_lower(hasher.finalize().as_slice())
}

/// Truncate to a one-line, max-len summary for trace narrative
/// output. Newlines collapse to spaces so the rendered trace stays
/// tabular. Mirrors `_truncate` in pipeline.py.
pub fn truncate_summary(text: &str, max_len: usize) -> String {
    let flat = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max_len {
        return flat;
    }
    let prefix: String = flat.chars().take(max_len.saturating_sub(1)).collect();
    format!("{prefix}…")
}

// ---------- private helpers ----------

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Serialize a serde_json::Value to bytes with sorted object keys.
/// `serde_json::to_vec` doesn't sort, so we walk + rebuild via a
/// sorted `BTreeMap`-backed Map. The rebuild is safe to call on the
/// trimmed plan (small JSON in practice — single plan per call).
fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    let canonical = canonicalize(value);
    serde_json::to_vec(&canonical).expect("canonical JSON serializes")
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // BTreeMap iteration is sorted; rebuild a Map in that
            // order so serde_json emits sorted keys.
            let mut sorted = Map::with_capacity(map.len());
            let keys: std::collections::BTreeSet<&String> = map.keys().collect();
            for key in keys {
                if let Some(v) = map.get(key) {
                    sorted.insert(key.clone(), canonicalize(v));
                }
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

mod hex {
    /// Lowercase hex encoding. Avoids pulling in the `hex` crate.
    pub fn encode_lower(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(ALPHABET[(b >> 4) as usize] as char);
            out.push(ALPHABET[(b & 0x0F) as usize] as char);
        }
        out
    }
}

#[allow(dead_code)]
fn dummy_use_json_to_silence_unused_import_warning() -> Value {
    json!(null)
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_ir::{Edit, Patch, PatchKind, PatchPlan};

    #[test]
    fn kind_counts_aggregates_by_name() {
        let names = ["fan_out", "chain_depth", "fan_out", "fan_out"];
        let out = kind_counts(names.iter().copied());
        assert_eq!(out["fan_out"], 3);
        assert_eq!(out["chain_depth"], 1);
    }

    #[test]
    fn kind_value_totals_sums_values() {
        let items = vec![
            ("fan_out", 15.0),
            ("fan_out", 8.0),
            ("chain_depth", 3.0),
        ];
        let out = kind_value_totals(items.iter().map(|(k, v)| (*k, *v)));
        assert_eq!(out["fan_out"], 23.0);
        assert_eq!(out["chain_depth"], 3.0);
    }

    #[test]
    fn total_cost_sums_values_with_zero_default() {
        assert_eq!(total_cost([1.0, 2.5, 0.5]), 4.0);
        assert_eq!(total_cost(std::iter::empty::<f64>()), 0.0);
    }

    #[test]
    fn regressed_strict_greater_than() {
        assert!(regressed(10.0, 12.0));
        assert!(!regressed(10.0, 10.0)); // equal is NOT regression
        assert!(!regressed(10.0, 8.0));
    }

    #[test]
    fn regression_detail_only_includes_increased_kinds() {
        let mut before = BTreeMap::new();
        before.insert("fan_out".into(), 10.0);
        before.insert("chain_depth".into(), 5.0);
        let mut after = BTreeMap::new();
        after.insert("fan_out".into(), 15.0); // grew → keep
        after.insert("chain_depth".into(), 3.0); // shrank → drop
        after.insert("new_kind".into(), 2.0); // appeared → keep
        let detail = regression_detail(&before, &after);
        assert_eq!(detail.len(), 2);
        assert_eq!(detail["fan_out"], 5.0);
        assert_eq!(detail["new_kind"], 2.0);
        assert!(!detail.contains_key("chain_depth"));
    }

    #[test]
    fn regression_detail_rounds_to_four_decimals() {
        let mut before = BTreeMap::new();
        before.insert("k".into(), 0.0);
        let mut after = BTreeMap::new();
        after.insert("k".into(), 0.123_456_789);
        let detail = regression_detail(&before, &after);
        assert_eq!(detail["k"], 0.1235); // rounded
    }

    fn make_plan(iteration: u32, parent_id: Option<&str>) -> PatchPlan {
        PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![Patch {
                id: "p1".into(),
                kind: PatchKind::Modify,
                path: "a.py".into(),
                rationale: "".into(),
                content: None,
                edits: vec![Edit::new("x", "y").with_context("", "\n")],
            }],
            target_files: vec!["a.py".into()],
            done: false,
            iteration,
            parent_id: parent_id.map(String::from),
        }
    }

    #[test]
    fn hash_plan_ignores_iteration_and_parent_id() {
        let h1 = hash_plan(&make_plan(0, None));
        let h2 = hash_plan(&make_plan(7, Some("plan-prev")));
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        // sanity check: hex chars only
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_plan_changes_when_content_changes() {
        let h1 = hash_plan(&make_plan(0, None));
        let mut p2 = make_plan(0, None);
        p2.goal = "different".into();
        let h2 = hash_plan(&p2);
        assert_ne!(h1, h2);
    }

    #[test]
    fn truncate_summary_collapses_whitespace_and_caps() {
        assert_eq!(truncate_summary("a   b\nc", 10), "a b c");
        assert_eq!(truncate_summary("a b c d e f", 5), "a b …");
    }
}
