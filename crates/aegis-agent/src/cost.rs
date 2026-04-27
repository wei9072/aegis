//! Cross-turn structural cost tracking — V3.3 differentiation #2.
//!
//! What aegis-mcp / `aegis check` give you per-edit, this layer
//! aggregates across an entire session: did the agent's cumulative
//! work raise the workspace's structural cost beyond a budget?
//!
//! "Cost" here means the sum of Ring 0.5 signal values for a file
//! (`fan_out` + `max_chain_depth` + future signals). The tracker is
//! source-agnostic — callers feed observations in; the tracker
//! aggregates baselines vs current state.
//!
//! Negative-space framing: the tracker observes and reports. It
//! never alters tool calls, never injects coaching strings into the
//! prompt, never restarts the session. The conversation runtime
//! consults the tracker between turns and decides whether the
//! per-session budget has been exceeded; if so, the session
//! terminates with `StoppedReason::CostBudgetExceeded`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct CostTracker {
    /// First cost we ever observed for each file. Acts as the
    /// "before" snapshot in cumulative regression accounting.
    baselines: BTreeMap<PathBuf, f64>,
    /// Most recent cost observed for each file.
    currents: BTreeMap<PathBuf, f64>,
}

impl CostTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an observation. The first observation for a path
    /// becomes its baseline; subsequent observations update the
    /// current cost only.
    pub fn observe(&mut self, path: impl Into<PathBuf>, cost: f64) {
        let path = path.into();
        self.baselines.entry(path.clone()).or_insert(cost);
        self.currents.insert(path, cost);
    }

    /// Total cumulative regression across the session.
    /// `Σ max(0, current - baseline)` per file. Files that improved
    /// (current < baseline) do NOT subtract from the total — the
    /// budget is about "how much worse have we made things",
    /// not net change.
    #[must_use]
    pub fn cumulative_regression(&self) -> f64 {
        self.baselines
            .iter()
            .map(|(path, baseline)| {
                let current = self.currents.get(path).copied().unwrap_or(*baseline);
                (current - baseline).max(0.0)
            })
            .sum()
    }

    /// Snapshot of all currently-tracked file costs (baseline +
    /// current). Useful for diagnostics + the eventual
    /// stalemate-detector hook in V3.5.
    #[must_use]
    pub fn snapshot(&self) -> Vec<CostEntry> {
        self.baselines
            .iter()
            .map(|(path, baseline)| CostEntry {
                path: path.clone(),
                baseline: *baseline,
                current: self.currents.get(path).copied().unwrap_or(*baseline),
            })
            .collect()
    }

    /// All paths the tracker has seen at least one observation for.
    #[must_use]
    pub fn observed_paths(&self) -> Vec<&Path> {
        self.baselines.keys().map(PathBuf::as_path).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CostEntry {
    pub path: PathBuf,
    pub baseline: f64,
    pub current: f64,
}

impl CostEntry {
    #[must_use]
    pub fn regression(&self) -> f64 {
        (self.current - self.baseline).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_observation_sets_baseline_and_current() {
        let mut t = CostTracker::new();
        t.observe("a.py", 10.0);
        let snap = t.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].baseline, 10.0);
        assert_eq!(snap[0].current, 10.0);
        assert_eq!(t.cumulative_regression(), 0.0);
    }

    #[test]
    fn second_observation_updates_current_only() {
        let mut t = CostTracker::new();
        t.observe("a.py", 10.0);
        t.observe("a.py", 15.0);
        let snap = t.snapshot();
        assert_eq!(snap[0].baseline, 10.0);
        assert_eq!(snap[0].current, 15.0);
        assert_eq!(t.cumulative_regression(), 5.0);
    }

    #[test]
    fn improvement_does_not_subtract_from_regression() {
        let mut t = CostTracker::new();
        t.observe("a.py", 10.0);
        t.observe("a.py", 5.0); // improved by 5
        // Improvement is GOOD but doesn't earn budget — we're
        // tracking degradation specifically.
        assert_eq!(t.cumulative_regression(), 0.0);
    }

    #[test]
    fn cumulative_regression_sums_per_file_increases() {
        let mut t = CostTracker::new();
        t.observe("a.py", 10.0);
        t.observe("b.py", 5.0);
        t.observe("a.py", 13.0); // +3
        t.observe("b.py", 4.0); // improved (no contribution)
        t.observe("c.py", 7.0); // new baseline
        assert_eq!(t.cumulative_regression(), 3.0);
    }
}
