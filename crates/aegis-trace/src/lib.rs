//! DecisionTrace — append-only, totally-ordered record of every gate
//! decision made during a single request.
//!
//! Pure data, no I/O, no logging. PyO3 wrappers live in
//! `aegis-pyshim`; downstream Rust crates depend on this directly.
//!
//! Mirrors `aegis/runtime/trace.py` exactly so the V1.0 cut-over is
//! a drop-in replacement.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Decision verbs. Layer names are open strings; verbs are not.
pub const PASS: &str = "pass";
pub const BLOCK: &str = "block";
pub const WARN: &str = "warn";
pub const OBSERVE: &str = "observe";

/// One decision recorded by one gate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionEvent {
    pub layer: String,
    pub decision: String,
    pub reason: String,
    pub signals: HashMap<String, f64>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: f64,
}

impl DecisionEvent {
    pub fn new(
        layer: impl Into<String>,
        decision: impl Into<String>,
        reason: impl Into<String>,
        signals: HashMap<String, f64>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            layer: layer.into(),
            decision: decision.into(),
            reason: reason.into(),
            signals,
            metadata,
            timestamp: now_seconds(),
        }
    }
}

/// Append-only event log. Helpers mirror the Python API shape.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DecisionTrace {
    pub events: Vec<DecisionEvent>,
}

impl DecisionTrace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn emit(
        &mut self,
        layer: impl Into<String>,
        decision: impl Into<String>,
        reason: impl Into<String>,
        signals: Option<HashMap<String, f64>>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> DecisionEvent {
        let event = DecisionEvent::new(
            layer,
            decision,
            reason,
            signals.unwrap_or_default(),
            metadata.unwrap_or_default(),
        );
        self.events.push(event.clone());
        event
    }

    pub fn by_layer(&self, layer: &str) -> Vec<DecisionEvent> {
        self.events.iter().filter(|e| e.layer == layer).cloned().collect()
    }

    pub fn by_decision(&self, decision: &str) -> Vec<DecisionEvent> {
        self.events
            .iter()
            .filter(|e| e.decision == decision)
            .cloned()
            .collect()
    }

    pub fn has_block(&self) -> bool {
        self.events.iter().any(|e| e.decision == BLOCK)
    }

    pub fn reasons(&self) -> Vec<String> {
        self.events.iter().map(|e| e.reason.clone()).collect()
    }
}

fn now_seconds() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_starts_empty() {
        let t = DecisionTrace::new();
        assert!(t.events.is_empty());
        assert!(!t.has_block());
    }

    #[test]
    fn emit_appends_in_order() {
        let mut t = DecisionTrace::new();
        t.emit("ring0", PASS, "syntax_valid", None, None);
        let mut sigs = HashMap::new();
        sigs.insert("fan_out".to_string(), 3.0);
        t.emit("ring0_5", OBSERVE, "fan_out", Some(sigs), None);
        assert_eq!(t.events.len(), 2);
        assert_eq!(t.events[0].layer, "ring0");
        assert_eq!(t.events[1].signals.get("fan_out"), Some(&3.0));
    }

    #[test]
    fn query_helpers() {
        let mut t = DecisionTrace::new();
        t.emit("ring0", PASS, "syntax_valid", None, None);
        t.emit("ring0", BLOCK, "circular_dependency", None, None);
        t.emit("ring0_5", OBSERVE, "fan_out", None, None);

        assert_eq!(t.by_layer("ring0").len(), 2);
        assert_eq!(t.by_decision(BLOCK).len(), 1);
        assert!(t.has_block());
        assert_eq!(
            t.reasons(),
            vec![
                "syntax_valid".to_string(),
                "circular_dependency".to_string(),
                "fan_out".to_string()
            ]
        );
    }
}
