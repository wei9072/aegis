//! Signal extraction aggregator.
//!
//! Filename retained from the V0.x PyO3 era for diff continuity;
//! the `_pyapi` suffix is now historical — the file is pure Rust as
//! of V1.10. A rename is a backlogged hygiene item.

use crate::signals::{fan_out_signal, chain_depth_signal};

/// Pure-data signal record. The Python-facing `Signal` struct that
/// previously lived here was deleted along with the `aegis-pyshim`
/// crate in V1.10; `SignalData` is what every Rust caller uses
/// (`validate.rs`, `scan.rs`, `runtime/context.rs`,
/// `agent/cost_observer_aegis.rs`, `cli/main.rs`).
#[derive(Clone, Debug)]
pub struct SignalData {
    pub name: String,
    pub value: f64,
    pub file_path: String,
    pub description: String,
}

/// Pure-Rust signal extraction.
pub fn extract_signals_native(filepath: &str) -> Result<Vec<SignalData>, String> {
    let fan_out = fan_out_signal(filepath)?;
    let depth = chain_depth_signal(filepath)?;

    Ok(vec![
        SignalData {
            name: "fan_out".to_string(),
            value: fan_out,
            file_path: filepath.to_string(),
            description: format!(
                "Number of unique external imports (fan-out = {})",
                fan_out as usize
            ),
        },
        SignalData {
            name: "max_chain_depth".to_string(),
            value: depth,
            file_path: filepath.to_string(),
            description: format!(
                "Maximum method/attribute chain depth (depth = {})",
                depth as usize
            ),
        },
    ])
}
