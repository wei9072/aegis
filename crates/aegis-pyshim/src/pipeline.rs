//! V1.3 full — Pipeline.run() loop body in Rust.
//!
//! The loop's *coordination* logic lives here; the Planner and the
//! ContextBuilder stay in Python because both are fundamentally
//! Python-shaped:
//!
//!   - `LLMPlanner.plan(ctx)` constructs the LLM prompt template
//!     from the rich `PlanContext` data; that template work belongs
//!     in Python.
//!   - `_build_context(task, root, scope, include_snippets)` walks
//!     the workspace and feeds it through `SignalLayer` +
//!     `GraphService` (Python-bound infrastructure today).
//!
//! Both are passed in as Python callables/objects; the Rust loop
//! invokes them through the GIL when needed. Every other step
//! (validation, execution, metric aggregation, sequence detection,
//! step-decision) calls native Rust through the `aegis-runtime` +
//! `aegis-decision` + `aegis-ir` crates.
//!
//! The Python `aegis/runtime/pipeline.py::_run_loop` becomes a
//! one-line dispatch into this function.

use std::path::PathBuf;

use aegis_decision::IterationEvent;
use aegis_ir::PatchPlan;
use aegis_runtime::{
    hash_plan, kind_counts, kind_value_totals, regressed as rs_regressed,
    regression_detail as rs_regression_detail, step_decision, total_cost as rs_total_cost,
    truncate_summary, Executor as RsExecutor, LoopState, PlanValidator as RsPlanValidator,
    StepDecision,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::ir::PyPatchPlan;
use crate::runtime::{
    PyExecutionResult, PyPipelineResult, PyPlanValidator, PyValidationError,
};

/// Drive the multi-turn refactor pipeline. Returns a populated
/// `PipelineResult`; the public Python `run()` wrapper attaches
/// `task_verdict` afterwards.
///
/// `planner` must expose `.plan(ctx) -> PatchPlan` (PyO3
/// `PyPatchPlan`). `ctx_builder` must be callable as
/// `ctx_builder(task, root, scope, include_snippets) -> PlanContext`,
/// returning any object that exposes `.signals: dict[str,
/// list[Signal]]` plus the mutable `previous_*` slots the loop sets
/// each iter.
#[pyfunction]
#[pyo3(signature = (
    task,
    root,
    planner,
    ctx_builder,
    scope=None,
    max_iters=3_usize,
    include_file_snippets=true,
    on_iteration=None,
))]
pub fn run_loop(
    py: Python<'_>,
    task: String,
    root: String,
    planner: PyObject,
    ctx_builder: PyObject,
    scope: Option<Vec<String>>,
    max_iters: usize,
    include_file_snippets: bool,
    on_iteration: Option<PyObject>,
) -> PyResult<Py<PyPipelineResult>> {
    let root_abs: PathBuf = std::fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(&root));
    let root_str = root_abs.display().to_string();

    let validator = make_validator(&root_str, scope.as_deref())?;
    let executor = RsExecutor::new(root_abs.clone());

    let mut ctx_obj = call_ctx_builder(
        py,
        &ctx_builder,
        &task,
        &root_str,
        scope.as_deref(),
        include_file_snippets,
    )?;

    // signals_before / prev metrics are derived from the very first
    // ctx, before any iteration runs.
    let signals_before_obj = read_signals(py, &ctx_obj)?;
    let mut prev_kind_counts = compute_kind_counts(py, &signals_before_obj)?;
    let mut prev_value_totals = compute_kind_value_totals(py, &signals_before_obj)?;

    let mut last_plan_hash: Option<String> = None;
    let mut last_plan_obj: Option<Py<PyPatchPlan>> = None;
    let mut last_result_obj: Option<Py<PyExecutionResult>> = None;
    let mut last_validation_errors: Vec<Py<PyValidationError>> = Vec::new();
    let mut last_regressed = false;
    let mut last_regression_detail: std::collections::BTreeMap<String, f64> =
        std::collections::BTreeMap::new();

    let mut state = LoopState::new();

    let result = Py::new(py, PyPipelineResult::new(
        py, false, 0, None, Some(signals_before_obj.clone_ref(py)),
        Some(signals_before_obj.clone_ref(py)), None, None, None, None,
    )?)?;

    // small closure-style helper: emit + step + maybe-terminate
    // (in inline form rather than a real closure since Rust closures
    // don't play nicely with `?` + heterogeneous returns here).

    for i in 0..max_iters {
        // 1. Update ctx.previous_* slots so the planner can read
        //    them when constructing this iter's prompt.
        write_previous_slots(
            py,
            &ctx_obj,
            last_plan_obj.as_ref(),
            &last_validation_errors,
            last_result_obj.as_ref(),
            last_regressed,
            &last_regression_detail,
        )?;

        // 2. planner.plan(ctx) → PatchPlan
        let plan_obj: Py<PyPatchPlan> = match call_planner(py, &planner, &ctx_obj) {
            Ok(p) => p,
            Err(err) => {
                let signals_now = read_signals(py, &ctx_obj)?;
                let mut r = result.borrow_mut(py);
                r.set_success(false);
                r.set_iterations(i as i64);
                r.set_final_plan(last_plan_obj);
                r.set_signals_after(Some(signals_now));
                r.set_error(Some(format!("planner failed: {}", err)));
                drop(r);
                return Ok(result);
            }
        };
        // 3. Set plan.iteration = i (mutates the Python object).
        plan_obj.as_ref(py).setattr("iteration", i as i64)?;

        // 4. hash_plan(plan); plan_repeated_now derivation.
        let (plan_hash, plan_done, plan_patches_count) = {
            let p = plan_obj.borrow(py);
            (
                hash_plan(p.inner_ref()),
                p.inner_ref().done,
                p.inner_ref().patches.len() as u32,
            )
        };
        let plan_id = plan_hash[..8].to_string();
        let plan_repeated_now = matches!(
            (&last_plan_hash, plan_done),
            (Some(prev), false) if *prev == plan_hash
        );
        last_plan_hash = Some(plan_hash);

        // 5. NOOP_DONE — planner explicitly declared completion with
        //    no patches; emit one event, skip _step (a coincidental
        //    value_totals stalemate must NOT override the explicit
        //    completion signal).
        if plan_done && plan_patches_count == 0 {
            let signals_now = read_signals(py, &ctx_obj)?;
            emit_event(
                py,
                on_iteration.as_ref(),
                i as u32,
                &plan_obj,
                &plan_id,
                Vec::<String>::new(),
                false,
                false,
                false,
                &signals_now,
                &prev_kind_counts,
                &prev_value_totals,
                None,
                false,
                false,
            )?;
            let mut r = result.borrow_mut(py);
            r.set_success(true);
            r.set_iterations(i as i64 + 1);
            r.set_final_plan(Some(plan_obj.clone_ref(py)));
            r.set_signals_after(Some(signals_now));
            drop(r);
            return Ok(result);
        }

        // 6. Validator.
        let validation_errs = {
            let p = plan_obj.borrow(py);
            validator.validate(p.inner_ref())
        };
        if !validation_errs.is_empty() {
            // Wrap each error in a PyValidationError for both the
            // event + the result.validation_errors field on early
            // termination.
            let py_errs: Vec<Py<PyValidationError>> = validation_errs
                .iter()
                .cloned()
                .map(|inner| Py::new(py, validation_error_from_inner(inner)))
                .collect::<PyResult<_>>()?;
            let err_strings: Vec<String> = validation_errs
                .iter()
                .map(format_validation_error)
                .collect();

            let signals_now = read_signals(py, &ctx_obj)?;
            let current_vt = compute_kind_value_totals(py, &signals_now)?;
            let decision =
                step_decision(&state, &current_vt, false, plan_repeated_now);
            emit_event(
                py,
                on_iteration.as_ref(),
                i as u32,
                &plan_obj,
                &plan_id,
                err_strings,
                false,
                false,
                false,
                &signals_now,
                &prev_kind_counts,
                &prev_value_totals,
                None,
                decision.stalemate_detected,
                decision.thrashing_detected,
            )?;
            state.record(current_vt, false);

            if let Some(reason) = decision.terminate_reason {
                let mut r = result.borrow_mut(py);
                r.set_success(false);
                r.set_iterations(i as i64 + 1);
                r.set_final_plan(Some(plan_obj.clone_ref(py)));
                r.set_signals_after(Some(signals_now));
                r.set_validation_errors(PyList::new(py, &py_errs))?;
                r.set_error(Some(reason.as_message()));
                drop(r);
                return Ok(result);
            }
            last_plan_obj = Some(plan_obj.clone_ref(py));
            last_validation_errors = py_errs;
            last_result_obj = None;
            last_regressed = false;
            continue;
        }

        // 7. Executor.apply.
        let exe_inner = {
            let p = plan_obj.borrow(py);
            executor.apply(p.inner_ref())
        };
        let exe_success = exe_inner.success;
        let mut exe_inner_owned = exe_inner;
        let exe_obj_factory =
            |inner: aegis_runtime::ExecutionResult| -> PyResult<Py<PyExecutionResult>> {
                Py::new(py, PyExecutionResult::from_inner(inner))
            };

        if !exe_success {
            let result_pyo = exe_obj_factory(exe_inner_owned.clone())?;
            let signals_now = read_signals(py, &ctx_obj)?;
            let current_vt = compute_kind_value_totals(py, &signals_now)?;
            let decision = step_decision(&state, &current_vt, false, plan_repeated_now);
            emit_event(
                py,
                on_iteration.as_ref(),
                i as u32,
                &plan_obj,
                &plan_id,
                Vec::<String>::new(),
                false,
                true,
                false,
                &signals_now,
                &prev_kind_counts,
                &prev_value_totals,
                None,
                decision.stalemate_detected,
                decision.thrashing_detected,
            )?;
            state.record(current_vt, false);

            if let Some(reason) = decision.terminate_reason {
                let mut r = result.borrow_mut(py);
                r.set_success(false);
                r.set_iterations(i as i64 + 1);
                r.set_final_plan(Some(plan_obj.clone_ref(py)));
                r.set_signals_after(Some(signals_now));
                r.set_execution_result(Some(result_pyo));
                r.set_error(Some(reason.as_message()));
                drop(r);
                return Ok(result);
            }
            last_plan_obj = Some(plan_obj.clone_ref(py));
            last_validation_errors = Vec::new();
            last_result_obj = Some(result_pyo);
            last_regressed = false;
            continue;
        }

        // 8. Re-build context to observe post-apply signals.
        let new_ctx = call_ctx_builder(
            py,
            &ctx_builder,
            &task,
            &root_str,
            scope.as_deref(),
            include_file_snippets,
        )?;
        let new_signals = read_signals(py, &new_ctx)?;
        let prev_signals = read_signals(py, &ctx_obj)?;

        // 9. Cost-based regression check.
        let prev_total = compute_total_cost(py, &prev_signals)?;
        let new_total = compute_total_cost(py, &new_signals)?;
        if rs_regressed(prev_total, new_total) {
            let prev_vt = compute_kind_value_totals(py, &prev_signals)?;
            let new_vt_full = compute_kind_value_totals(py, &new_signals)?;
            let detail = rs_regression_detail(&prev_vt, &new_vt_full);
            // Roll back; mark on the inner ExecutionResult so the
            // returned PyExecutionResult carries the bit for callers.
            executor
                .rollback_result(&exe_inner_owned)
                .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
            exe_inner_owned.rolled_back = true;
            let result_pyo = exe_obj_factory(exe_inner_owned)?;

            let signals_now = prev_signals.clone_ref(py);
            let current_vt = compute_kind_value_totals(py, &signals_now)?;
            let decision = step_decision(&state, &current_vt, true, plan_repeated_now);
            emit_event(
                py,
                on_iteration.as_ref(),
                i as u32,
                &plan_obj,
                &plan_id,
                Vec::<String>::new(),
                true,
                true,
                true,
                &signals_now,
                &prev_kind_counts,
                &prev_value_totals,
                Some(&detail),
                decision.stalemate_detected,
                decision.thrashing_detected,
            )?;
            state.record(current_vt, true);

            if let Some(reason) = decision.terminate_reason {
                let mut r = result.borrow_mut(py);
                r.set_success(false);
                r.set_iterations(i as i64 + 1);
                r.set_final_plan(Some(plan_obj.clone_ref(py)));
                r.set_signals_after(Some(signals_now));
                r.set_execution_result(Some(result_pyo));
                r.set_error(Some(reason.as_message()));
                drop(r);
                return Ok(result);
            }
            last_plan_obj = Some(plan_obj.clone_ref(py));
            last_validation_errors = Vec::new();
            last_result_obj = Some(result_pyo);
            last_regressed = true;
            last_regression_detail = detail;
            continue;
        }

        // 10. Successful apply with no regression — promote new ctx.
        ctx_obj = new_ctx;
        let signals_now = new_signals;
        let current_vt = compute_kind_value_totals(py, &signals_now)?;
        let decision = step_decision(&state, &current_vt, false, plan_repeated_now);
        emit_event(
            py,
            on_iteration.as_ref(),
            i as u32,
            &plan_obj,
            &plan_id,
            Vec::<String>::new(),
            true,
            false,
            false,
            &signals_now,
            &prev_kind_counts,
            &prev_value_totals,
            None,
            decision.stalemate_detected,
            decision.thrashing_detected,
        )?;
        state.record(current_vt.clone(), false);

        if let Some(reason) = decision.terminate_reason {
            let result_pyo = exe_obj_factory(exe_inner_owned)?;
            let mut r = result.borrow_mut(py);
            r.set_success(false);
            r.set_iterations(i as i64 + 1);
            r.set_final_plan(Some(plan_obj.clone_ref(py)));
            r.set_signals_after(Some(signals_now));
            r.set_execution_result(Some(result_pyo));
            r.set_error(Some(reason.as_message()));
            drop(r);
            return Ok(result);
        }

        prev_kind_counts = compute_kind_counts(py, &signals_now)?;
        prev_value_totals = current_vt;
        let result_pyo = exe_obj_factory(exe_inner_owned)?;
        last_plan_obj = Some(plan_obj.clone_ref(py));
        last_validation_errors = Vec::new();
        last_result_obj = Some(result_pyo.clone_ref(py));
        last_regressed = false;
        last_regression_detail.clear();

        if plan_done {
            let mut r = result.borrow_mut(py);
            r.set_success(true);
            r.set_iterations(i as i64 + 1);
            r.set_final_plan(Some(plan_obj.clone_ref(py)));
            r.set_signals_after(Some(signals_now));
            r.set_execution_result(Some(result_pyo));
            drop(r);
            return Ok(result);
        }
    }

    // 11. Loop ran all the way through — max_iters without explicit
    //     completion. Mirror the V0.x error message verbatim
    //     (downstream tooling pattern-matches on it).
    let signals_now = read_signals(py, &ctx_obj)?;
    let mut r = result.borrow_mut(py);
    r.set_success(false);
    r.set_iterations(max_iters as i64);
    r.set_final_plan(last_plan_obj);
    r.set_signals_after(Some(signals_now));
    r.set_execution_result(last_result_obj);
    r.set_validation_errors(PyList::new(py, &last_validation_errors))?;
    r.set_error(Some(
        "max iterations reached without planner declaring done".to_string(),
    ));
    drop(r);
    Ok(result)
}

// ---------- helpers ----------

fn make_validator(root: &str, scope: Option<&[String]>) -> PyResult<RsPlanValidator> {
    let mut v = RsPlanValidator::new(root);
    if let Some(scope) = scope {
        v = v
            .with_scope(scope.to_vec())
            .map_err(pyo3::exceptions::PyValueError::new_err)?;
    }
    Ok(v)
}

fn validation_error_from_inner(inner: aegis_runtime::ValidationError) -> PyValidationError {
    PyValidationError::from_inner(inner)
}

/// Format mirrors the Python `str(ValidationError)` dataclass repr —
/// good enough for the trace `validation_errors: list[str]` field.
/// Tests don't assert exact bytes here.
fn format_validation_error(e: &aegis_runtime::ValidationError) -> String {
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

fn call_planner(
    py: Python<'_>,
    planner: &PyObject,
    ctx: &PyObject,
) -> PyResult<Py<PyPatchPlan>> {
    let result = planner.call_method1(py, "plan", (ctx,))?;
    let plan: Py<PyPatchPlan> = result.extract(py)?;
    Ok(plan)
}

fn call_ctx_builder(
    py: Python<'_>,
    ctx_builder: &PyObject,
    task: &str,
    root: &str,
    scope: Option<&[String]>,
    include_snippets: bool,
) -> PyResult<PyObject> {
    let scope_obj: PyObject = match scope {
        Some(s) => PyList::new(py, s).into(),
        None => py.None(),
    };
    let args = (task, root, scope_obj, include_snippets);
    ctx_builder.call1(py, args)
}

fn read_signals(py: Python<'_>, ctx: &PyObject) -> PyResult<PyObject> {
    Ok(ctx.getattr(py, "signals")?)
}

fn write_previous_slots(
    py: Python<'_>,
    ctx: &PyObject,
    previous_plan: Option<&Py<PyPatchPlan>>,
    previous_errors: &[Py<PyValidationError>],
    previous_result: Option<&Py<PyExecutionResult>>,
    previous_regressed: bool,
    previous_regression_detail: &std::collections::BTreeMap<String, f64>,
) -> PyResult<()> {
    let none = py.None();
    ctx.setattr(
        py,
        "previous_plan",
        previous_plan.map(|p| p.clone_ref(py).into_py(py)).unwrap_or(none),
    )?;
    let errs = PyList::new(
        py,
        previous_errors.iter().map(|e| e.clone_ref(py).into_py(py)),
    );
    ctx.setattr(py, "previous_errors", errs)?;
    let none2 = py.None();
    ctx.setattr(
        py,
        "previous_result",
        previous_result.map(|r| r.clone_ref(py).into_py(py)).unwrap_or(none2),
    )?;
    ctx.setattr(py, "previous_regressed", previous_regressed)?;
    let detail = PyDict::new(py);
    for (k, v) in previous_regression_detail {
        detail.set_item(k, v)?;
    }
    ctx.setattr(py, "previous_regression_detail", detail)?;
    Ok(())
}

fn compute_kind_counts(
    py: Python<'_>,
    signals: &PyObject,
) -> PyResult<std::collections::BTreeMap<String, u64>> {
    let signals = signals.as_ref(py);
    let d: &PyDict = signals.downcast()?;
    let mut names = Vec::new();
    for (_path, sig_list) in d.iter() {
        let l: &PyList = sig_list.downcast()?;
        for sig in l.iter() {
            names.push(sig.getattr("name")?.extract::<String>()?);
        }
    }
    Ok(kind_counts(names.iter().map(|s| s.as_str())))
}

fn compute_kind_value_totals(
    py: Python<'_>,
    signals: &PyObject,
) -> PyResult<std::collections::BTreeMap<String, f64>> {
    let signals = signals.as_ref(py);
    let d: &PyDict = signals.downcast()?;
    let mut items: Vec<(String, f64)> = Vec::new();
    for (_path, sig_list) in d.iter() {
        let l: &PyList = sig_list.downcast()?;
        for sig in l.iter() {
            let name = sig.getattr("name")?.extract::<String>()?;
            let value = sig.getattr("value")?.extract::<f64>()?;
            items.push((name, value));
        }
    }
    Ok(kind_value_totals(items.iter().map(|(k, v)| (k.as_str(), *v))))
}

fn compute_total_cost(py: Python<'_>, signals: &PyObject) -> PyResult<f64> {
    let signals = signals.as_ref(py);
    let d: &PyDict = signals.downcast()?;
    let mut values = Vec::new();
    for (_path, sig_list) in d.iter() {
        let l: &PyList = sig_list.downcast()?;
        for sig in l.iter() {
            values.push(sig.getattr("value")?.extract::<f64>()?);
        }
    }
    Ok(rs_total_cost(values.into_iter()))
}

#[allow(clippy::too_many_arguments)]
fn emit_event(
    py: Python<'_>,
    on_iteration: Option<&PyObject>,
    iteration: u32,
    plan: &Py<PyPatchPlan>,
    plan_id: &str,
    validation_errors: Vec<String>,
    applied: bool,
    rolled_back: bool,
    regressed: bool,
    signals: &PyObject,
    prev_kind_counts: &std::collections::BTreeMap<String, u64>,
    prev_value_totals: &std::collections::BTreeMap<String, f64>,
    regression_detail: Option<&std::collections::BTreeMap<String, f64>>,
    stalemate_detected: bool,
    thrashing_detected: bool,
) -> PyResult<()> {
    let kind_counts_now = compute_kind_counts(py, signals)?;
    let value_totals_now = compute_kind_value_totals(py, signals)?;
    let signals_total: u64 = kind_counts_now.values().sum();
    let signals_by_kind: std::collections::BTreeMap<String, i64> = kind_counts_now
        .iter()
        .map(|(k, v)| (k.clone(), *v as i64))
        .collect();
    let signal_delta_vs_prev = subtract_i64(&signals_by_kind, prev_kind_counts);
    let signal_value_delta_vs_prev = subtract_f64(&value_totals_now, prev_value_totals);

    let (plan_goal, plan_strategy, plan_done, plan_patches) = {
        let p = plan.borrow(py);
        let inner = p.inner_ref();
        (
            truncate_summary(&inner.goal, 200),
            truncate_summary(&inner.strategy, 240),
            inner.done,
            inner.patches.len() as u32,
        )
    };

    let inner = IterationEvent {
        iteration,
        plan_id: plan_id.to_string(),
        plan_goal,
        plan_strategy,
        plan_done,
        plan_patches,
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
    };
    let event_py = Py::new(py, crate::decision::PyIterationEvent::from_inner(inner))?;
    if let Some(cb) = on_iteration {
        cb.call1(py, (event_py,))?;
    }
    Ok(())
}

fn subtract_i64(
    now: &std::collections::BTreeMap<String, i64>,
    prev: &std::collections::BTreeMap<String, u64>,
) -> std::collections::BTreeMap<String, i64> {
    let mut out = std::collections::BTreeMap::new();
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
    now: &std::collections::BTreeMap<String, f64>,
    prev: &std::collections::BTreeMap<String, f64>,
) -> std::collections::BTreeMap<String, f64> {
    let mut out = std::collections::BTreeMap::new();
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

// `_` placeholders silence clippy on unused imports we still need
// for documentation / future call sites.
#[allow(dead_code)]
fn _silence_unused(_p: PyPatchPlan, _v: PyPlanValidator, _s: StepDecision, _pp: PatchPlan) {}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(run_loop, m)?)?;
    Ok(())
}
