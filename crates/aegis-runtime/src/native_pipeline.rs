//! Native Rust pipeline loop — V1.9 binary entry point.
//!
//! This is the pure-Rust sibling of `aegis-pyshim::pipeline::run_loop`.
//! Same coordination logic; same step-decision precedence; same
//! IterationEvent shape per turn. The difference is the seams:
//!
//!   - `pyshim::pipeline::run_loop` calls Python objects through PyO3
//!     (LLMPlanner / `_build_context` stay in Python).
//!   - `native_pipeline::run_pipeline` (this module) takes Rust trait
//!     objects (`&dyn Planner`, `&dyn ContextBuilder`), so a binary
//!     can wire pure-Rust LLMPlanner + WorkspaceContextBuilder and
//!     drop Python entirely.
//!
//! The `aegis-cli` binary uses this for its `pipeline run` subcommand.

use std::collections::BTreeMap;
use std::path::Path;

use aegis_decision::IterationEvent;
use aegis_ir::PatchPlan;

use crate::context::{PlanContext, Signal};
use crate::executor::{ExecutionResult, Executor};
use crate::loop_step::{step_decision, LoopState};
use crate::metrics::{
    hash_plan, kind_counts, kind_value_totals, regressed as cost_regressed,
    regression_detail, total_cost, truncate_summary,
};
use crate::planner_trait::{ContextBuilder, Planner};
use crate::validator::PlanValidator;

/// Outcome of one full pipeline run. Mirrors the V0.x Python
/// `PipelineResult` dataclass field-for-field.
#[derive(Debug, Default)]
pub struct PipelineResult {
    pub success: bool,
    pub iterations: u32,
    pub final_plan: Option<PatchPlan>,
    pub signals_before: BTreeMap<String, Vec<Signal>>,
    pub signals_after: BTreeMap<String, Vec<Signal>>,
    pub error: Option<String>,
    pub validation_errors: Vec<crate::validator::ValidationError>,
    pub execution_result: Option<ExecutionResult>,
}

/// Run-time knobs.
#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub max_iters: u32,
    pub include_file_snippets: bool,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            max_iters: 3,
            include_file_snippets: true,
        }
    }
}

/// Drive the multi-turn refactor pipeline. Pure Rust; no PyO3.
///
/// `on_iteration` fires once per loop iteration with the freshly
/// built `IterationEvent`; pass `|_| {}` to ignore.
pub fn run_pipeline(
    task: &str,
    root: &Path,
    scope: Option<&[String]>,
    planner: &dyn Planner,
    ctx_builder: &dyn ContextBuilder,
    options: &PipelineOptions,
    mut on_iteration: impl FnMut(&IterationEvent),
) -> PipelineResult {
    let mut ctx = ctx_builder.build(task, root, scope, options.include_file_snippets);
    let signals_before = ctx.signals.clone();

    let validator = make_validator(root, scope);
    let executor = Executor::new(root.to_path_buf());

    let mut prev_kind_counts = compute_kind_counts(&ctx.signals);
    let mut prev_value_totals = compute_value_totals(&ctx.signals);

    let mut last_plan_hash: Option<String> = None;
    let mut last_plan: Option<PatchPlan> = None;
    let mut last_result: Option<ExecutionResult> = None;
    let mut last_validation_errors: Vec<crate::validator::ValidationError> = Vec::new();
    let mut last_regressed = false;
    let mut last_regression_detail: BTreeMap<String, f64> = BTreeMap::new();

    let mut state = LoopState::new();

    for i in 0..options.max_iters {
        // Refresh ctx.previous_* slots for this iter's planner call.
        ctx.previous_plan = last_plan.clone();
        ctx.previous_errors = last_validation_errors.clone();
        ctx.previous_result = last_result.clone();
        ctx.previous_regressed = last_regressed;
        ctx.previous_regression_detail = last_regression_detail.clone();

        let plan = match planner.plan(&mut ctx) {
            Ok(p) => p,
            Err(e) => {
                return PipelineResult {
                    success: false,
                    iterations: i,
                    final_plan: last_plan,
                    signals_before,
                    signals_after: ctx.signals,
                    error: Some(format!("planner failed: {e}")),
                    validation_errors: last_validation_errors,
                    execution_result: last_result,
                };
            }
        };
        let mut plan = plan;
        plan.iteration = i;

        let plan_hash = hash_plan(&plan);
        let plan_id = plan_hash[..8].to_string();
        let plan_repeated_now = matches!(
            (&last_plan_hash, plan.done),
            (Some(prev), false) if *prev == plan_hash
        );
        last_plan_hash = Some(plan_hash);

        // NOOP_DONE — planner explicitly declared completion with no patches.
        if plan.done && plan.patches.is_empty() {
            let event = build_iteration_event(
                i,
                &plan,
                &plan_id,
                Vec::new(),
                false,
                false,
                false,
                &ctx.signals,
                &prev_kind_counts,
                &prev_value_totals,
                None,
                false,
                false,
            );
            on_iteration(&event);
            return PipelineResult {
                success: true,
                iterations: i + 1,
                final_plan: Some(plan),
                signals_before,
                signals_after: ctx.signals,
                ..Default::default()
            };
        }

        // Validation gate.
        let validation_errs = validator.validate(&plan);
        if !validation_errs.is_empty() {
            let err_strings: Vec<String> = validation_errs
                .iter()
                .map(format_validation_error)
                .collect();
            let current_vt = compute_value_totals(&ctx.signals);
            let decision =
                step_decision(&state, &current_vt, false, plan_repeated_now);
            let event = build_iteration_event(
                i,
                &plan,
                &plan_id,
                err_strings,
                false,
                false,
                false,
                &ctx.signals,
                &prev_kind_counts,
                &prev_value_totals,
                None,
                decision.stalemate_detected,
                decision.thrashing_detected,
            );
            on_iteration(&event);
            state.record(current_vt, false);

            if let Some(reason) = decision.terminate_reason {
                return PipelineResult {
                    success: false,
                    iterations: i + 1,
                    final_plan: Some(plan),
                    signals_before,
                    signals_after: ctx.signals,
                    error: Some(reason.as_message()),
                    validation_errors: validation_errs,
                    execution_result: None,
                };
            }
            last_plan = Some(plan);
            last_validation_errors = validation_errs;
            last_result = None;
            last_regressed = false;
            continue;
        }

        // Executor apply.
        let mut exe_result = executor.apply(&plan);
        if !exe_result.success {
            let current_vt = compute_value_totals(&ctx.signals);
            let decision = step_decision(&state, &current_vt, false, plan_repeated_now);
            let event = build_iteration_event(
                i,
                &plan,
                &plan_id,
                Vec::new(),
                false,
                true,
                false,
                &ctx.signals,
                &prev_kind_counts,
                &prev_value_totals,
                None,
                decision.stalemate_detected,
                decision.thrashing_detected,
            );
            on_iteration(&event);
            state.record(current_vt, false);

            if let Some(reason) = decision.terminate_reason {
                return PipelineResult {
                    success: false,
                    iterations: i + 1,
                    final_plan: Some(plan),
                    signals_before,
                    signals_after: ctx.signals,
                    error: Some(reason.as_message()),
                    validation_errors: Vec::new(),
                    execution_result: Some(exe_result),
                };
            }
            last_plan = Some(plan);
            last_validation_errors = Vec::new();
            last_result = Some(exe_result);
            last_regressed = false;
            continue;
        }

        // Re-build context to observe post-apply signals.
        let new_ctx =
            ctx_builder.build(task, root, scope, options.include_file_snippets);

        let prev_total = compute_total_cost(&ctx.signals);
        let new_total = compute_total_cost(&new_ctx.signals);
        if cost_regressed(prev_total, new_total) {
            let prev_vt = compute_value_totals(&ctx.signals);
            let new_vt = compute_value_totals(&new_ctx.signals);
            let detail = regression_detail(&prev_vt, &new_vt);
            let _ = executor.rollback_result(&exe_result);
            exe_result.rolled_back = true;

            let signals_now = ctx.signals.clone();
            let current_vt = compute_value_totals(&signals_now);
            let decision = step_decision(&state, &current_vt, true, plan_repeated_now);
            let event = build_iteration_event(
                i,
                &plan,
                &plan_id,
                Vec::new(),
                true,
                true,
                true,
                &signals_now,
                &prev_kind_counts,
                &prev_value_totals,
                Some(&detail),
                decision.stalemate_detected,
                decision.thrashing_detected,
            );
            on_iteration(&event);
            state.record(current_vt, true);

            if let Some(reason) = decision.terminate_reason {
                return PipelineResult {
                    success: false,
                    iterations: i + 1,
                    final_plan: Some(plan),
                    signals_before,
                    signals_after: signals_now,
                    error: Some(reason.as_message()),
                    validation_errors: Vec::new(),
                    execution_result: Some(exe_result),
                };
            }
            last_plan = Some(plan);
            last_validation_errors = Vec::new();
            last_result = Some(exe_result);
            last_regressed = true;
            last_regression_detail = detail;
            continue;
        }

        // Successful apply, no regression — promote new_ctx.
        ctx = new_ctx;
        let current_vt = compute_value_totals(&ctx.signals);
        let decision = step_decision(&state, &current_vt, false, plan_repeated_now);
        let event = build_iteration_event(
            i,
            &plan,
            &plan_id,
            Vec::new(),
            true,
            false,
            false,
            &ctx.signals,
            &prev_kind_counts,
            &prev_value_totals,
            None,
            decision.stalemate_detected,
            decision.thrashing_detected,
        );
        on_iteration(&event);
        state.record(current_vt.clone(), false);

        if let Some(reason) = decision.terminate_reason {
            return PipelineResult {
                success: false,
                iterations: i + 1,
                final_plan: Some(plan),
                signals_before,
                signals_after: ctx.signals,
                error: Some(reason.as_message()),
                validation_errors: Vec::new(),
                execution_result: Some(exe_result),
            };
        }

        prev_kind_counts = compute_kind_counts(&ctx.signals);
        prev_value_totals = current_vt;
        let plan_done = plan.done;
        let exe_clone = exe_result.clone();
        last_plan = Some(plan);
        last_validation_errors = Vec::new();
        last_result = Some(exe_clone);
        last_regressed = false;
        last_regression_detail.clear();

        if plan_done {
            return PipelineResult {
                success: true,
                iterations: i + 1,
                final_plan: last_plan,
                signals_before,
                signals_after: ctx.signals,
                execution_result: Some(exe_result),
                ..Default::default()
            };
        }
    }

    PipelineResult {
        success: false,
        iterations: options.max_iters,
        final_plan: last_plan,
        signals_before,
        signals_after: ctx.signals,
        error: Some(
            "max iterations reached without planner declaring done".to_string(),
        ),
        validation_errors: last_validation_errors,
        execution_result: last_result,
    }
}

// ---------- helpers ----------

fn make_validator(root: &Path, scope: Option<&[String]>) -> PlanValidator {
    let mut v = PlanValidator::new(root.to_path_buf());
    if let Some(s) = scope {
        if !s.is_empty() {
            v = v.with_scope(s.to_vec()).expect("scope path resolves under root");
        }
    }
    v
}

fn format_validation_error(e: &crate::validator::ValidationError) -> String {
    let pid = e.patch_id.as_deref().unwrap_or("");
    let idx = e
        .edit_index
        .map(|i| format!(" edit#{i}"))
        .unwrap_or_default();
    format!(
        "ValidationError(kind={:?}, patch_id={:?}{}, message={:?})",
        e.kind.as_str(),
        pid,
        idx,
        e.message
    )
}

fn compute_kind_counts(signals: &BTreeMap<String, Vec<Signal>>) -> BTreeMap<String, u64> {
    kind_counts(signals.values().flatten().map(|s| s.name.as_str()))
}

fn compute_value_totals(signals: &BTreeMap<String, Vec<Signal>>) -> BTreeMap<String, f64> {
    kind_value_totals(
        signals
            .values()
            .flatten()
            .map(|s| (s.name.as_str(), s.value)),
    )
}

fn compute_total_cost(signals: &BTreeMap<String, Vec<Signal>>) -> f64 {
    total_cost(signals.values().flatten().map(|s| s.value))
}

#[allow(clippy::too_many_arguments)]
fn build_iteration_event(
    iteration: u32,
    plan: &PatchPlan,
    plan_id: &str,
    validation_errors: Vec<String>,
    applied: bool,
    rolled_back: bool,
    regressed: bool,
    signals: &BTreeMap<String, Vec<Signal>>,
    prev_kind_counts: &BTreeMap<String, u64>,
    prev_value_totals: &BTreeMap<String, f64>,
    regression_detail: Option<&BTreeMap<String, f64>>,
    stalemate_detected: bool,
    thrashing_detected: bool,
) -> IterationEvent {
    let counts_now = compute_kind_counts(signals);
    let value_totals_now = compute_value_totals(signals);
    let signals_total: u64 = counts_now.values().sum();
    let signals_by_kind: BTreeMap<String, i64> = counts_now
        .iter()
        .map(|(k, v)| (k.clone(), *v as i64))
        .collect();
    let signal_delta_vs_prev = subtract_i64(&signals_by_kind, prev_kind_counts);
    let signal_value_delta_vs_prev =
        subtract_f64(&value_totals_now, prev_value_totals);

    IterationEvent {
        iteration,
        plan_id: plan_id.to_string(),
        plan_goal: truncate_summary(&plan.goal, 200),
        plan_strategy: truncate_summary(&plan.strategy, 240),
        plan_done: plan.done,
        plan_patches: plan.patches.len() as u32,
        validation_passed: validation_errors.is_empty(),
        validation_errors,
        applied,
        rolled_back,
        regressed,
        signals_total,
        signals_by_kind,
        signal_delta_vs_prev,
        signal_value_totals: value_totals_now,
        signal_value_delta_vs_prev,
        regression_detail: regression_detail.cloned().unwrap_or_default(),
        stalemate_detected,
        thrashing_detected,
    }
}

fn subtract_i64(
    now: &BTreeMap<String, i64>,
    prev: &BTreeMap<String, u64>,
) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    let mut keys: std::collections::BTreeSet<&String> = now.keys().collect();
    keys.extend(prev.keys());
    for key in keys {
        let n = now.get(key).copied().unwrap_or(0);
        let p = prev.get(key).copied().unwrap_or(0) as i64;
        out.insert(key.clone(), n - p);
    }
    out
}

fn subtract_f64(
    now: &BTreeMap<String, f64>,
    prev: &BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let mut keys: std::collections::BTreeSet<&String> = now.keys().collect();
    keys.extend(prev.keys());
    for key in keys {
        let n = now.get(key).copied().unwrap_or(0.0);
        let p = prev.get(key).copied().unwrap_or(0.0);
        let delta = (n - p) * 10_000.0;
        out.insert(key.clone(), delta.round() / 10_000.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextOptions;
    use crate::planner_trait::{PlannerError, WorkspaceContextBuilder};
    use aegis_ir::{Edit, Patch, PatchKind};
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct StubPlanner {
        responses: Mutex<Vec<PatchPlan>>,
    }

    impl Planner for StubPlanner {
        fn plan(&self, _ctx: &mut PlanContext) -> Result<PatchPlan, PlannerError> {
            let mut r = self.responses.lock().unwrap();
            if r.is_empty() {
                return Err(PlannerError::Failed("no more plans".into()));
            }
            Ok(r.remove(0))
        }
    }

    #[test]
    fn single_iteration_apply_done_succeeds() {
        let td = tempdir().unwrap();
        std::fs::write(td.path().join("a.py"), "header\noriginal\nfooter\n").unwrap();
        let plan = PatchPlan {
            goal: "rename".into(),
            strategy: "single MODIFY".into(),
            patches: vec![Patch {
                id: "p1".into(),
                kind: PatchKind::Modify,
                path: "a.py".into(),
                rationale: "the rename".into(),
                content: None,
                edits: vec![Edit::new("original", "renamed")
                    .with_context("header\n", "\nfooter")],
            }],
            target_files: vec!["a.py".into()],
            done: true,
            iteration: 0,
            parent_id: None,
        };
        let planner = StubPlanner {
            responses: Mutex::new(vec![plan]),
        };
        let cb = WorkspaceContextBuilder;
        let mut events = Vec::new();
        let result = run_pipeline(
            "rename",
            td.path(),
            None,
            &planner,
            &cb,
            &PipelineOptions {
                max_iters: 2,
                include_file_snippets: false,
            },
            |ev| events.push(ev.clone()),
        );
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.iterations, 1);
        assert_eq!(events.len(), 1);
        assert_eq!(
            std::fs::read_to_string(td.path().join("a.py")).unwrap(),
            "header\nrenamed\nfooter\n",
        );
    }

    #[test]
    fn noop_done_terminates_without_executor_run() {
        let td = tempdir().unwrap();
        let plan = PatchPlan {
            goal: "nothing".into(),
            strategy: "no patches".into(),
            patches: vec![],
            target_files: vec![],
            done: true,
            iteration: 0,
            parent_id: None,
        };
        let planner = StubPlanner {
            responses: Mutex::new(vec![plan]),
        };
        let cb = WorkspaceContextBuilder;
        let result = run_pipeline(
            "noop",
            td.path(),
            None,
            &planner,
            &cb,
            &PipelineOptions::default(),
            |_| {},
        );
        assert!(result.success);
        assert_eq!(result.iterations, 1);
        assert!(result.execution_result.is_none());
    }

    #[test]
    fn validation_failure_stops_with_error_in_result() {
        let td = tempdir().unwrap();
        // MODIFY without context — schema-invalid plan.
        let plan = PatchPlan {
            goal: "bad".into(),
            strategy: "no context".into(),
            patches: vec![Patch {
                id: "p1".into(),
                kind: PatchKind::Modify,
                path: "a.py".into(),
                rationale: "".into(),
                content: None,
                edits: vec![Edit::new("x", "y")],
            }],
            target_files: vec![],
            done: true,
            iteration: 0,
            parent_id: None,
        };
        let planner = StubPlanner {
            responses: Mutex::new(vec![plan.clone(), plan.clone(), plan]),
        };
        let cb = WorkspaceContextBuilder;
        let opts = ContextOptions::default();
        let _ = opts; // doc-link; clippy
        let result = run_pipeline(
            "bad",
            td.path(),
            None,
            &planner,
            &cb,
            &PipelineOptions {
                max_iters: 3,
                include_file_snippets: false,
            },
            |_| {},
        );
        assert!(!result.success);
    }

    #[test]
    fn planner_error_aborts_with_error_message() {
        let td = tempdir().unwrap();
        let planner = StubPlanner {
            responses: Mutex::new(Vec::new()), // first call fails
        };
        let cb = WorkspaceContextBuilder;
        let result = run_pipeline(
            "anything",
            td.path(),
            None,
            &planner,
            &cb,
            &PipelineOptions::default(),
            |_| {},
        );
        assert!(!result.success);
        assert!(result.error.unwrap().starts_with("planner failed"));
    }
}
