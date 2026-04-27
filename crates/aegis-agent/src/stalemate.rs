//! Session-level stalemate detection — V3.4 / V3.5 differentiation #4.
//!
//! Detects when the agent is making no structural progress: across N
//! successive iterations, the workspace's total structural cost
//! hasn't changed. Indicates the LLM is going in circles inside its
//! own loop.
//!
//! When detected, the runtime terminates the turn with
//! `StoppedReason::StalemateDetected` — reuses the V1 named pattern.
//! No retry, no coaching — the user decides whether to start a fresh
//! session with a refined task.
//!
//! **Thrashing detection** (V1-era `THRASHING_DETECTED` for "≥2
//! consecutive regression rollbacks") is mechanism-only here in V3:
//! the conversation agent doesn't *roll back*, so the precondition
//! never fires. The trip-wire stays in `aegis-runtime` for the
//! pipeline-mode loop; this module only does state-stalemate.

/// How many successive identical cost totals trigger
/// `StoppedReason::StalemateDetected`. Mirrors
/// `aegis_runtime::loop_step::STATE_STALEMATE_THRESHOLD` so the
/// pipeline loop and the agent loop have the same cadence.
pub const STATE_STALEMATE_THRESHOLD: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub enum StalemateVerdict {
    Continue,
    StateStalemate,
}

#[derive(Clone, Debug, Default)]
pub struct StalemateDetector {
    recent_totals: Vec<f64>,
}

impl StalemateDetector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the latest cost total. Returns the verdict for the
    /// resulting series.
    pub fn record(&mut self, total: f64) -> StalemateVerdict {
        self.recent_totals.push(total);
        // Keep only the most recent THRESHOLD entries — old data
        // can't matter once it falls out of the window.
        if self.recent_totals.len() > STATE_STALEMATE_THRESHOLD {
            let drop = self.recent_totals.len() - STATE_STALEMATE_THRESHOLD;
            self.recent_totals.drain(0..drop);
        }
        if self.recent_totals.len() >= STATE_STALEMATE_THRESHOLD
            && self.recent_totals.windows(2).all(|w| w[0] == w[1])
        {
            return StalemateVerdict::StateStalemate;
        }
        StalemateVerdict::Continue
    }

    pub fn reset(&mut self) {
        self.recent_totals.clear();
    }

    pub fn recent_totals(&self) -> &[f64] {
        &self.recent_totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_observation_continues() {
        let mut d = StalemateDetector::new();
        assert_eq!(d.record(10.0), StalemateVerdict::Continue);
    }

    #[test]
    fn two_identical_totals_continue() {
        let mut d = StalemateDetector::new();
        d.record(10.0);
        assert_eq!(d.record(10.0), StalemateVerdict::Continue);
    }

    #[test]
    fn three_identical_totals_trigger_stalemate() {
        let mut d = StalemateDetector::new();
        d.record(10.0);
        d.record(10.0);
        assert_eq!(d.record(10.0), StalemateVerdict::StateStalemate);
    }

    #[test]
    fn movement_resets_stalemate_window() {
        let mut d = StalemateDetector::new();
        d.record(10.0);
        d.record(10.0);
        d.record(11.0); // movement
        d.record(11.0);
        assert_eq!(d.record(11.0), StalemateVerdict::StateStalemate);
    }

    #[test]
    fn fluctuating_totals_never_stalemate() {
        let mut d = StalemateDetector::new();
        for v in [1.0, 2.0, 1.0, 2.0, 1.0, 2.0] {
            assert_eq!(d.record(v), StalemateVerdict::Continue);
        }
    }
}
