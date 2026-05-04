//! Signal extraction aggregator.
//!
//! Filename retained from the V0.x PyO3 era for diff continuity;
//! the `_pyapi` suffix is now historical — the file is pure Rust as
//! of V1.10. A rename is a backlogged hygiene item.

use serde::{Deserialize, Serialize};

use crate::signals::{chain_depth_signal, fan_out_signal, smell_counts};

/// S7.1 — How seriously the cost-regression layer should treat a
/// regression of this signal.
///
/// - `Block`: the signal is "verifiably bad" — its growth has
///   essentially no legitimate use case. A regression triggers BLOCK.
///   Examples: empty_handler, unreachable_stmt, mutable_default_arg,
///   suspicious_literal, unresolved_local_import, test_count_lost.
///
/// - `Warn`: the signal is "heuristically suspicious" — usually bad
///   but a refactoring legitimately raises it (adding a guard clause
///   raises cyclomatic; an entry file legitimately raises fan_out).
///   A regression is reported but does NOT trigger BLOCK on its own.
///   Examples: fan_out, max_chain_depth, cyclomatic_complexity,
///   nesting_depth.
///
/// - `Info`: pure observation. Reported in signal_deltas but never
///   flagged. Currently unused; reserved for project-statistics
///   signals where there is no inherent direction (yet).
///
/// Why this exists: aegis's earlier sum-then-per-signal evolution
/// treated all signals as block-level, which produced false positives
/// on legitimate refactors that shifted shape (e.g., splitting a god
/// function into modules raised fan_out → BLOCK). Severity-tagged
/// signals let the discipline ("only reject what is verifiably bad")
/// hold for the strict ones while the heuristic ones become advice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalSeverity {
    Block,
    Warn,
    Info,
}

impl SignalSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalSeverity::Block => "block",
            SignalSeverity::Warn => "warn",
            SignalSeverity::Info => "info",
        }
    }
}

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
    pub severity: SignalSeverity,
}

/// Severity registry. Looking it up by name lets `validate.rs`
/// classify a signal that came in via cost regression. Used for
/// constructing SignalData in this module too.
pub fn severity_for(name: &str) -> SignalSeverity {
    match name {
        // Verifiably bad — these have no legitimate growth case.
        "empty_handler_count"
        | "unreachable_stmt_count"
        | "mutable_default_arg_count"
        | "shadowed_local_count"
        | "suspicious_literal_count"
        | "unresolved_local_import_count"
        | "unfinished_marker_count"
        | "test_count_lost" => SignalSeverity::Block,
        // Heuristically suspicious — fan_out / chain_depth /
        // cyclomatic / nesting_depth are valid signals to report
        // but should never solely block a change. They false-positive
        // on entry files, on legitimate guard clauses, on adding new
        // branches for new business logic, etc.
        "fan_out"
        | "max_chain_depth"
        | "cyclomatic_complexity"
        | "nesting_depth" => SignalSeverity::Warn,
        // S7.4 — pure visibility, never blocks or warns alone.
        "member_access_count" | "type_leakage_count" => SignalSeverity::Info,
        // S7.5 — heuristic Demeter signal; warn-level so reviewers
        // see it but it doesn't block by itself.
        "cross_module_chain_count" => SignalSeverity::Warn,
        // Default: unknown signals get Warn so they're at least
        // visible without making the verdict noisy.
        _ => SignalSeverity::Warn,
    }
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

    fn make(name: &str, value: f64, filepath: &str, description: String) -> SignalData {
        SignalData {
            name: name.to_string(),
            value,
            file_path: filepath.to_string(),
            description,
            severity: severity_for(name),
        }
    }
    Ok(vec![
        make(
            "fan_out",
            fan_out,
            filepath,
            format!("Unique external imports (fan-out = {})", fan_out as usize),
        ),
        make(
            "max_chain_depth",
            depth,
            filepath,
            format!("Max method/attribute chain depth ({})", depth as usize),
        ),
        make(
            "empty_handler_count",
            smells.empty_handler_count,
            filepath,
            format!(
                "Empty catch/except handlers ({})",
                smells.empty_handler_count as usize
            ),
        ),
        make(
            "unfinished_marker_count",
            smells.unfinished_marker_count,
            filepath,
            format!(
                "TODO/FIXME/todo!() markers ({})",
                smells.unfinished_marker_count as usize
            ),
        ),
        make(
            "unreachable_stmt_count",
            smells.unreachable_stmt_count,
            filepath,
            format!(
                "Statements after return/throw/break/continue ({})",
                smells.unreachable_stmt_count as usize
            ),
        ),
        make(
            "cyclomatic_complexity",
            smells.cyclomatic_complexity,
            filepath,
            format!(
                "Branching constructs ({})",
                smells.cyclomatic_complexity as usize
            ),
        ),
        make(
            "nesting_depth",
            smells.nesting_depth,
            filepath,
            format!("Max nesting depth ({})", smells.nesting_depth as usize),
        ),
        make(
            "suspicious_literal_count",
            smells.suspicious_literal_count,
            filepath,
            format!(
                "Hardcoded secrets / localhost / local paths ({})",
                smells.suspicious_literal_count as usize
            ),
        ),
        make(
            "mutable_default_arg_count",
            smells.mutable_default_arg_count,
            filepath,
            format!(
                "Mutable default args like def f(x=[]) ({})",
                smells.mutable_default_arg_count as usize
            ),
        ),
        make(
            "shadowed_local_count",
            smells.shadowed_local_count,
            filepath,
            format!(
                "Same-scope variable rebinds ({})",
                smells.shadowed_local_count as usize
            ),
        ),
        // S4.1: test_count is INVERSE — losing tests is the smell.
        // We negate it before participating in cost regression so the
        // delta layer treats test_count 10→7 as growth.
        make(
            "test_count_lost",
            -smells.test_count,
            filepath,
            format!(
                "Negated test count (lower = more tests = better). Current = {}",
                smells.test_count as usize
            ),
        ),
        // S7.4: member-access count and type-leakage. Both info-
        // level — they're context for reviewers, not block triggers.
        make(
            "member_access_count",
            smells.member_access_count,
            filepath,
            format!(
                "Attribute / member-access expressions ({})",
                smells.member_access_count as usize
            ),
        ),
        make(
            "type_leakage_count",
            smells.type_leakage_count,
            filepath,
            format!(
                "External-type references in public signatures ({})",
                smells.type_leakage_count as usize
            ),
        ),
        // S7.5: chains depth >= 3 whose root looks external. Closer
        // proxy for "Demeter violation across modules" than
        // max_chain_depth alone, but still a heuristic.
        make(
            "cross_module_chain_count",
            smells.cross_module_chain_count,
            filepath,
            format!(
                "Chains depth>=3 with externally-rooted receiver ({})",
                smells.cross_module_chain_count as usize
            ),
        ),
    ])
}
