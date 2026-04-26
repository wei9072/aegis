//! `IterationEvent` — one iteration's outcome, in a shape stable
//! enough for JSON serialisation and run-to-run diffing.
//!
//! Mirrors the V0.x Python `aegis.runtime.pipeline.IterationEvent`
//! dataclass field-for-field. `silent_done_contradiction` is a
//! derived property (not stored), matching the Python `@property`.
//!
//! Two parallel signal views, intentionally redundant:
//!   - `signals_by_kind` / `signal_delta_vs_prev`: how many *instances*
//!     of each signal kind exist (≈ how many files carry that signal).
//!     Useful for "did a new file pick up an issue?" questions.
//!   - `signal_value_totals` / `signal_value_delta_vs_prev`: the
//!     summed *values* across files (a file with `fan_out=15` and a
//!     file with `fan_out=8` give a fan_out total of 23). This is
//!     what answers "did the pipeline make the metric better or
//!     worse?", which the instance-count view alone cannot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IterationEvent {
    pub iteration: u32,
    pub plan_id: String,
    /// Planner's restatement of the task. Truncated by callers to
    /// keep traces tabular.
    #[serde(default)]
    pub plan_goal: String,
    /// Planner's approach for this iteration. Truncated by callers.
    #[serde(default)]
    pub plan_strategy: String,
    pub plan_done: bool,
    pub plan_patches: u32,
    pub validation_passed: bool,
    #[serde(default)]
    pub validation_errors: Vec<String>,
    pub applied: bool,
    pub rolled_back: bool,
    pub regressed: bool,
    #[serde(default)]
    pub signals_total: u64,
    #[serde(default)]
    pub signals_by_kind: BTreeMap<String, i64>,
    #[serde(default)]
    pub signal_delta_vs_prev: BTreeMap<String, i64>,
    #[serde(default)]
    pub signal_value_totals: BTreeMap<String, f64>,
    #[serde(default)]
    pub signal_value_delta_vs_prev: BTreeMap<String, f64>,
    /// Per-kind cost growth that triggered rollback this iteration,
    /// if any. Empty on iterations that didn't regress.
    #[serde(default)]
    pub regression_detail: BTreeMap<String, f64>,
    /// Sequence-level meta-decision, set by the loop after observing
    /// the event history. When true this iteration's pattern resolves
    /// to STALEMATE_DETECTED.
    pub stalemate_detected: bool,
    /// Sibling of `stalemate_detected`. THRASHING_DETECTED dominates.
    pub thrashing_detected: bool,
}

impl IterationEvent {
    /// The Planner declared done but the patch never made it to disk.
    /// Computed (matches the Python `@property`) so two events with
    /// the same boolean state always derive the same flag.
    pub fn silent_done_contradiction(&self) -> bool {
        self.plan_done && !self.applied && self.plan_patches > 0
    }
}
