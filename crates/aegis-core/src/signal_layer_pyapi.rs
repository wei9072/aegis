//! Signal extraction aggregator.
//!
//! Filename retained from the V0.x PyO3 era for diff continuity;
//! the `_pyapi` suffix is now historical — the file is pure Rust as
//! of V1.10. A rename is a backlogged hygiene item.

use crate::signals::{chain_depth_signal, fan_out_signal, smell_counts};

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

/// Pure-Rust signal extraction. Returns the union of:
/// - Ring 0.5 structural metrics (fan_out, max_chain_depth)
/// - Phase 2 LLM-failure smell counters (empty handlers, TODOs,
///   unreachable code, cyclomatic complexity, nesting depth,
///   suspicious literals)
///
/// All counters participate in cost-aware regression — block fires
/// only when a counter goes UP relative to old_content.
pub fn extract_signals_native(filepath: &str) -> Result<Vec<SignalData>, String> {
    let fan_out = fan_out_signal(filepath)?;
    let depth = chain_depth_signal(filepath)?;
    let smells = smell_counts(filepath).unwrap_or_default();

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
        SignalData {
            name: "empty_handler_count".to_string(),
            value: smells.empty_handler_count,
            file_path: filepath.to_string(),
            description: format!(
                "Empty catch/except handlers (count = {})",
                smells.empty_handler_count as usize
            ),
        },
        SignalData {
            name: "unfinished_marker_count".to_string(),
            value: smells.unfinished_marker_count,
            file_path: filepath.to_string(),
            description: format!(
                "TODO/FIXME/todo!() markers (count = {})",
                smells.unfinished_marker_count as usize
            ),
        },
        SignalData {
            name: "unreachable_stmt_count".to_string(),
            value: smells.unreachable_stmt_count,
            file_path: filepath.to_string(),
            description: format!(
                "Statements after a return/throw/break/continue (count = {})",
                smells.unreachable_stmt_count as usize
            ),
        },
        SignalData {
            name: "cyclomatic_complexity".to_string(),
            value: smells.cyclomatic_complexity,
            file_path: filepath.to_string(),
            description: format!(
                "Branching constructs (count = {})",
                smells.cyclomatic_complexity as usize
            ),
        },
        SignalData {
            name: "nesting_depth".to_string(),
            value: smells.nesting_depth,
            file_path: filepath.to_string(),
            description: format!(
                "Max nesting depth (depth = {})",
                smells.nesting_depth as usize
            ),
        },
        SignalData {
            name: "suspicious_literal_count".to_string(),
            value: smells.suspicious_literal_count,
            file_path: filepath.to_string(),
            description: format!(
                "Hardcoded secrets/localhost/local-paths (count = {})",
                smells.suspicious_literal_count as usize
            ),
        },
        SignalData {
            name: "mutable_default_arg_count".to_string(),
            value: smells.mutable_default_arg_count,
            file_path: filepath.to_string(),
            description: format!(
                "Mutable default args like def f(x=[]) (count = {})",
                smells.mutable_default_arg_count as usize
            ),
        },
        SignalData {
            name: "shadowed_local_count".to_string(),
            value: smells.shadowed_local_count,
            file_path: filepath.to_string(),
            description: format!(
                "Same-scope variable rebinds (count = {})",
                smells.shadowed_local_count as usize
            ),
        },
        // S4.1: test_count is INVERSE — losing tests is the smell.
        // We negate it before participating in cost regression so the
        // delta layer treats "test_count goes from 10 → 7" as growth.
        SignalData {
            name: "test_count_lost".to_string(),
            value: -smells.test_count,
            file_path: filepath.to_string(),
            description: format!(
                "Negated test count (smaller = more tests = better). \
                 Current test_count={}",
                smells.test_count as usize
            ),
        },
    ])
}
