"""
Refactor pipeline: Planner -> Validator -> Executor -> Re-analyze loop.

Stop conditions (any one):
  - planner.done=True and no regression
  - planner produces plan identical to previous iteration (stalemate)
  - max_iters reached
Regression (total signal count increased) triggers rollback for that
iteration; loop continues if budget remains.
"""
from __future__ import annotations

import hashlib
import json
import os
from collections import Counter
from dataclasses import dataclass, field
from typing import Any, Callable
from pathlib import Path

from aegis.agents.llm_adapter import LLMProvider
from aegis.agents.planner import LLMPlanner, PlanContext
from aegis.analysis.signals import SignalLayer
from aegis.core.bindings import Signal, get_imports
from aegis.graph.service import GraphService
from aegis.ir.patch import PatchPlan, plan_to_dict
from aegis.runtime.decision_pattern import DecisionPattern, derive_pattern
from aegis.runtime.executor import ExecutionResult, Executor
from aegis.runtime.task_verifier import TaskVerdict, TaskVerifier, apply_verifier
from aegis.runtime.validator import PlanValidator, ValidationError


@dataclass(frozen=True)
class IterationEvent:
    """One iteration's outcome, in a shape stable enough for JSON
    serialisation and run-to-run diffing.

    Captures *what the pipeline decided this turn*, not the full plan
    content (kept out for compactness — `plan_id` lets two runs be
    compared without storing every patch). Multi-turn scenario runners
    consume these to render trajectories and to assert convergence.

    Two parallel signal views, intentionally redundant:
      - `signals_by_kind` / `signal_delta_vs_prev`: how many *instances*
        of each signal kind exist (≈ how many files carry that signal).
        Useful for "did a new file pick up an issue?" questions.
      - `signal_value_totals` / `signal_value_delta_vs_prev`: the
        summed *values* across files (a file with `fan_out=15` and a
        file with `fan_out=8` give a fan_out total of 23). This is
        what answers "did the pipeline make the metric better or
        worse?", which the instance-count view alone cannot.
    """

    iteration: int
    plan_id: str                 # 8-char prefix of plan hash; stable across runs of identical plans
    plan_goal: str = ""          # planner's restatement of the task
    plan_strategy: str = ""      # planner's approach for this iteration
    plan_done: bool = False      # planner declared "task complete"
    plan_patches: int = 0        # number of patches in the plan
    validation_passed: bool = False
    validation_errors: list[str] = field(default_factory=list)
    applied: bool = False        # executor ran and succeeded
    rolled_back: bool = False    # executor ran but rolled back (regression or failure)
    regressed: bool = False      # post-apply signal count > pre-apply
    signals_total: int = 0
    signals_by_kind: dict[str, int] = field(default_factory=dict)
    signal_delta_vs_prev: dict[str, int] = field(default_factory=dict)
    signal_value_totals: dict[str, float] = field(default_factory=dict)
    signal_value_delta_vs_prev: dict[str, float] = field(default_factory=dict)
    # Per-kind cost growth that triggered rollback this iteration, if
    # any. Empty dict on iterations that didn't regress. Captured here
    # (not just on the next ctx.previous_regression_detail) so the
    # JSON snapshot records *which* cost grew on the rolled-back step.
    regression_detail: dict[str, float] = field(default_factory=dict)
    # Sequence-level meta-decisions, set by pipeline._run_loop after
    # observing the event history. When either is True, this
    # iteration's decision_pattern resolves to STALEMATE_DETECTED /
    # THRASHING_DETECTED, overriding the per-iteration mechanical
    # shape — the meta-observation is the more honest label.
    stalemate_detected: bool = False
    thrashing_detected: bool = False

    @property
    def silent_done_contradiction(self) -> bool:
        """The Planner declared done but the patch never made it to disk.
        Pipeline correctly ignored the flag, but a downstream observer
        (CLI / report) wants to surface this loudly."""
        return self.plan_done and not self.applied and self.plan_patches > 0

    @property
    def decision_pattern(self) -> DecisionPattern:
        """Which named shape this iteration's outcome falls into.
        Derived purely from the boolean flags above — see
        `aegis.runtime.decision_pattern.derive_pattern` for the logic.
        Scenario expectations and trace summaries consume this."""
        return derive_pattern(self)

    def to_dict(self) -> dict[str, Any]:
        return {
            "iteration": self.iteration,
            "plan_id": self.plan_id,
            "plan_goal": self.plan_goal,
            "plan_strategy": self.plan_strategy,
            "plan_done": self.plan_done,
            "plan_patches": self.plan_patches,
            "validation_passed": self.validation_passed,
            "validation_errors": list(self.validation_errors),
            "applied": self.applied,
            "rolled_back": self.rolled_back,
            "regressed": self.regressed,
            "signals_total": self.signals_total,
            "signals_by_kind": dict(self.signals_by_kind),
            "signal_delta_vs_prev": dict(self.signal_delta_vs_prev),
            "signal_value_totals": dict(self.signal_value_totals),
            "signal_value_delta_vs_prev": dict(self.signal_value_delta_vs_prev),
            "regression_detail": dict(self.regression_detail),
            "stalemate_detected": self.stalemate_detected,
            "thrashing_detected": self.thrashing_detected,
            "silent_done_contradiction": self.silent_done_contradiction,
            "decision_pattern": self.decision_pattern.value,
        }


IterationCallback = Callable[[IterationEvent], None]


@dataclass
class PipelineResult:
    success: bool
    iterations: int
    final_plan: PatchPlan | None = None
    signals_before: dict[str, list[Signal]] = field(default_factory=dict)
    signals_after: dict[str, list[Signal]] = field(default_factory=dict)
    error: str | None = None
    validation_errors: list[ValidationError] = field(default_factory=list)
    execution_result: ExecutionResult | None = None
    # Layer C — populated after the loop terminates by the public
    # `run()` wrapper. Always present (TaskPattern.NO_VERIFIER when no
    # verifier was provided). Never read by the loop or the planner.
    task_verdict: TaskVerdict | None = None


def run(
    task: str,
    root: str,
    provider: LLMProvider,
    scope: list[str] | None = None,
    max_iters: int = 3,
    include_file_snippets: bool = True,
    on_iteration: IterationCallback | None = None,
    verifier: TaskVerifier | None = None,
) -> PipelineResult:
    """Drive the pipeline loop, then apply the Layer C verifier.

    The verifier (if provided) runs **after** the loop terminates,
    inspects the final workspace, and produces a TaskVerdict that is
    attached to the returned PipelineResult. Verifier output is never
    fed back into the loop, never reaches the planner prompt, and
    never produces a new IterationEvent — these isolation rules are
    what keep Aegis a decision-system rather than a goal-seeker.
    See `aegis/runtime/task_verifier.py` for the full design rules.
    """
    captured: list[IterationEvent] = []

    def _capturing_cb(ev: IterationEvent) -> None:
        captured.append(ev)
        if on_iteration is not None:
            on_iteration(ev)

    result = _run_loop(
        task=task,
        root=root,
        provider=provider,
        scope=scope,
        max_iters=max_iters,
        include_file_snippets=include_file_snippets,
        on_iteration=_capturing_cb,
    )
    result.task_verdict = apply_verifier(
        verifier=verifier,
        workspace=Path(root).resolve(),
        trace=captured,
        pipeline_done=result.success,
        iterations_run=result.iterations,
    )
    return result


def _run_loop(
    task: str,
    root: str,
    provider: LLMProvider,
    scope: list[str] | None = None,
    max_iters: int = 3,
    include_file_snippets: bool = True,
    on_iteration: IterationCallback | None = None,
) -> PipelineResult:
    root_abs = str(Path(root).resolve())
    planner = LLMPlanner(provider)
    validator = PlanValidator(root_abs, scope=scope)
    executor = Executor(root_abs)

    ctx = _build_context(task, root_abs, scope, include_file_snippets)
    signals_before = ctx.signals
    prev_kind_counts = _kind_counts(ctx.signals)
    prev_value_totals = _kind_value_totals(ctx.signals)

    last_plan_hash: str | None = None
    last_plan: PatchPlan | None = None
    last_result: ExecutionResult | None = None
    last_errors: list[ValidationError] = []
    last_regressed = False
    last_regression_detail: dict[str, float] = {}

    # Sequence-level detector history. Appended after every emitted
    # IterationEvent; consumed by _is_state_stalemate / _is_thrashing
    # before the *next* emit decides whether this iteration's pattern
    # should be the meta-pattern (STALEMATE_DETECTED / THRASHING_DETECTED).
    value_totals_history: list[dict[str, float]] = []
    regressed_history: list[bool] = []

    def _step(
        *,
        applied: bool,
        rolled_back: bool,
        regressed: bool,
        validation_errors_str: list[str],
        regression_detail: dict[str, float] | None = None,
        plan_repeated_now: bool = False,
    ) -> tuple[IterationEvent, str | None]:
        """Compute sequence-level flags, build + emit the event, update
        history, return (event, terminate_reason).

        terminate_reason is None when the loop should keep going.
        Otherwise it's a human-readable explanation that goes into
        PipelineResult.error.

        `plan_repeated_now` is a *supporting* signal for stalemate,
        not a primary trigger — see `_is_plan_repeat_stalemate` for
        the rationale. Stalemate fires when *either* state has been
        unchanged for `_STATE_STALEMATE_THRESHOLD` iters *or* the
        plan is byte-identical to the previous one AND state hasn't
        moved since.
        """
        current_vt = _kind_value_totals(ctx.signals)
        state_stalemate = _is_state_stalemate(value_totals_history, current_vt)
        plan_repeat_stalemate = _is_plan_repeat_stalemate(
            plan_repeated_now, value_totals_history, current_vt,
        )
        thrash = _is_thrashing(regressed_history, regressed)
        stalemate = state_stalemate or plan_repeat_stalemate

        event = _emit_iteration(
            on_iteration,
            iteration=i,
            plan=plan,
            plan_id=plan_id,
            validation_errors=validation_errors_str,
            applied=applied,
            rolled_back=rolled_back,
            regressed=regressed,
            ctx_signals=ctx.signals,
            prev_kind_counts=prev_kind_counts,
            prev_value_totals=prev_value_totals,
            regression_detail=regression_detail,
            stalemate_detected=stalemate,
            thrashing_detected=thrash,
        )
        value_totals_history.append(current_vt)
        regressed_history.append(regressed)

        # Thrashing dominates stalemate when both fire — see
        # derive_pattern's order-of-checks for the same reasoning.
        if thrash:
            return event, (
                f"thrashing detected — {_THRASHING_THRESHOLD} consecutive "
                f"regression rollbacks; further iterations would burn budget"
            )
        if stalemate:
            if plan_repeat_stalemate and not state_stalemate:
                return event, (
                    "stalemate — planner repeated identical plan AND "
                    "signal_value_totals unchanged since last iter"
                )
            return event, (
                f"state stalemate — signal_value_totals unchanged for "
                f"{_STATE_STALEMATE_THRESHOLD} iters; loop is making no progress"
            )
        return event, None

    for i in range(max_iters):
        ctx.previous_plan = last_plan
        ctx.previous_errors = last_errors
        ctx.previous_result = last_result
        ctx.previous_regressed = last_regressed
        ctx.previous_regression_detail = dict(last_regression_detail)

        try:
            plan = planner.plan(ctx)
        except Exception as e:
            return PipelineResult(
                success=False,
                iterations=i,
                final_plan=last_plan,
                signals_before=signals_before,
                signals_after=ctx.signals,
                error=f"planner failed: {e}",
            )
        plan.iteration = i

        plan_hash = _hash_plan(plan)
        plan_id = plan_hash[:8]
        # Plan-repeat is a supporting signal — _step will combine it
        # with state-movement check to decide stalemate. We no longer
        # short-circuit on plan repeat alone (LLMs can rephrase same
        # intent / reuse same wording with different intent — single
        # signal too noisy). See _is_plan_repeat_stalemate for the
        # full rationale.
        plan_repeated_now = (
            last_plan_hash is not None
            and plan_hash == last_plan_hash
            and not plan.done
        )
        last_plan_hash = plan_hash

        if plan.done and not plan.patches:
            # NOOP_DONE — planner positively declared completion.
            # Skip _step so a coincidental value_totals stalemate
            # doesn't override the explicit completion signal.
            # See aegis_core_framing_negative_space memory: refusal
            # is for degradation, not for declarations of completion.
            _emit_iteration(
                on_iteration,
                iteration=i,
                plan=plan,
                plan_id=plan_id,
                validation_errors=[],
                applied=False,
                rolled_back=False,
                regressed=False,
                ctx_signals=ctx.signals,
                prev_kind_counts=prev_kind_counts,
                prev_value_totals=prev_value_totals,
            )
            return PipelineResult(
                success=True,
                iterations=i + 1,
                final_plan=plan,
                signals_before=signals_before,
                signals_after=ctx.signals,
            )

        errors = validator.validate(plan)
        if errors:
            _, term = _step(
                applied=False, rolled_back=False, regressed=False,
                validation_errors_str=[str(e) for e in errors],
                plan_repeated_now=plan_repeated_now,
            )
            if term is not None:
                return PipelineResult(
                    success=False, iterations=i + 1, final_plan=plan,
                    signals_before=signals_before, signals_after=ctx.signals,
                    validation_errors=errors, error=term,
                )
            last_plan, last_errors, last_result, last_regressed = plan, errors, None, False
            continue

        result = executor.apply(plan)
        if not result.success:
            _, term = _step(
                applied=False, rolled_back=True,  # executor returned failure, state restored
                regressed=False,
                validation_errors_str=[],
                plan_repeated_now=plan_repeated_now,
            )
            if term is not None:
                return PipelineResult(
                    success=False, iterations=i + 1, final_plan=plan,
                    signals_before=signals_before, signals_after=ctx.signals,
                    execution_result=result, error=term,
                )
            last_plan, last_errors, last_result, last_regressed = plan, [], result, False
            continue

        new_ctx = _build_context(task, root_abs, scope, include_file_snippets)
        if _regressed(ctx.signals, new_ctx.signals):
            detail = _regression_detail(ctx.signals, new_ctx.signals)
            executor.rollback_result(result)
            result.rolled_back = True
            _, term = _step(
                applied=True, rolled_back=True, regressed=True,
                validation_errors_str=[],
                regression_detail=detail,
                plan_repeated_now=plan_repeated_now,
            )
            if term is not None:
                return PipelineResult(
                    success=False, iterations=i + 1, final_plan=plan,
                    signals_before=signals_before, signals_after=ctx.signals,
                    execution_result=result, error=term,
                )
            last_plan, last_errors, last_result, last_regressed = plan, [], result, True
            last_regression_detail = detail
            continue

        ctx = new_ctx
        _, term = _step(
            applied=True, rolled_back=False, regressed=False,
            validation_errors_str=[],
            plan_repeated_now=plan_repeated_now,
        )
        if term is not None:
            return PipelineResult(
                success=False, iterations=i + 1, final_plan=plan,
                signals_before=signals_before, signals_after=ctx.signals,
                execution_result=result, error=term,
            )
        prev_kind_counts = _kind_counts(ctx.signals)
        prev_value_totals = _kind_value_totals(ctx.signals)
        last_plan, last_errors, last_result, last_regressed = plan, [], result, False
        last_regression_detail = {}

        if plan.done:
            return PipelineResult(
                success=True,
                iterations=i + 1,
                final_plan=plan,
                signals_before=signals_before,
                signals_after=ctx.signals,
                execution_result=result,
            )

    return PipelineResult(
        success=False,
        iterations=max_iters,
        final_plan=last_plan,
        signals_before=signals_before,
        signals_after=ctx.signals,
        execution_result=last_result,
        validation_errors=last_errors,
        error="max iterations reached without planner declaring done",
    )


def _emit_iteration(
    cb: IterationCallback | None,
    *,
    iteration: int,
    plan: PatchPlan,
    plan_id: str,
    validation_errors: list[str],
    applied: bool,
    rolled_back: bool,
    regressed: bool,
    ctx_signals: dict[str, list[Signal]],
    prev_kind_counts: dict[str, int],
    prev_value_totals: dict[str, float],
    regression_detail: dict[str, float] | None = None,
    stalemate_detected: bool = False,
    thrashing_detected: bool = False,
) -> IterationEvent:
    """Build the IterationEvent (always — even if cb is None) and
    optionally pass it to the callback. Returns the event so the
    caller can append to its detector history lists.

    Stalemate / thrashing flags are computed by the caller from
    history; this helper just plumbs them into the event so
    derive_pattern() resolves to the right meta-pattern.
    """
    kind_counts = _kind_counts(ctx_signals)
    value_totals = _kind_value_totals(ctx_signals)
    count_delta = {
        k: kind_counts.get(k, 0) - prev_kind_counts.get(k, 0)
        for k in set(kind_counts) | set(prev_kind_counts)
    }
    value_delta = {
        k: round(value_totals.get(k, 0.0) - prev_value_totals.get(k, 0.0), 4)
        for k in set(value_totals) | set(prev_value_totals)
    }
    event = IterationEvent(
        iteration=iteration,
        plan_id=plan_id,
        plan_goal=_truncate(getattr(plan, "goal", "") or "", 200),
        plan_strategy=_truncate(getattr(plan, "strategy", "") or "", 240),
        plan_done=bool(plan.done),
        plan_patches=len(plan.patches),
        validation_passed=not validation_errors,
        validation_errors=list(validation_errors),
        applied=applied,
        rolled_back=rolled_back,
        regressed=regressed,
        signals_total=sum(len(v) for v in ctx_signals.values()),
        signals_by_kind=kind_counts,
        signal_delta_vs_prev=count_delta,
        signal_value_totals=value_totals,
        signal_value_delta_vs_prev=value_delta,
        regression_detail=dict(regression_detail or {}),
        stalemate_detected=stalemate_detected,
        thrashing_detected=thrashing_detected,
    )
    if cb is not None:
        cb(event)
    return event


# Default thresholds for sequence-level detectors. Tuned conservatively:
#   - state stalemate: 3 iters with identical value_totals. Threshold
#     of 2 would false-positive on a legitimate single noop iter; 3
#     means "two consecutive iters of no movement" which is a real
#     decision-loop signal.
#   - thrashing: 2 consecutive REGRESSION_ROLLBACK events. Rollback
#     is rare enough that two-in-a-row is itself the alarm.
_STATE_STALEMATE_THRESHOLD = 3
_THRASHING_THRESHOLD = 2


# Detector helpers ported to Rust in V1.3 (re-export pattern). The
# Python signatures are preserved for backward compatibility — pinned
# by `tests/test_decision_pattern.py::test_*_helper_threshold_*`.
# Threshold args still accepted but only the Rust default is used;
# callers that need to vary thresholds were never wired (no V0.x
# call site changes the default).
def _is_state_stalemate(
    history: list[dict[str, float]],
    current_value_totals: dict[str, float],
    threshold: int = _STATE_STALEMATE_THRESHOLD,
) -> bool:
    if threshold != _STATE_STALEMATE_THRESHOLD:
        # No V0.x caller varies this; the Rust impl hardcodes
        # threshold=3. If a future caller needs custom thresholds,
        # surface that as a Cargo trait method and re-export.
        if len(history) < threshold - 1:
            return False
        recent = history[-(threshold - 1):]
        return all(t == current_value_totals for t in recent)
    from aegis._core import is_state_stalemate
    return is_state_stalemate(history, current_value_totals)


def _is_thrashing(
    history: list[bool],
    regressed_now: bool,
    threshold: int = _THRASHING_THRESHOLD,
) -> bool:
    if threshold != _THRASHING_THRESHOLD:
        if not regressed_now:
            return False
        if len(history) < threshold - 1:
            return False
        return all(history[-(threshold - 1):])
    from aegis._core import is_thrashing
    return is_thrashing(history, regressed_now)


def _is_plan_repeat_stalemate(
    plan_repeated_now: bool,
    value_totals_history: list[dict[str, float]],
    current_value_totals: dict[str, float],
) -> bool:
    from aegis._core import is_plan_repeat_stalemate
    return is_plan_repeat_stalemate(
        plan_repeated_now, value_totals_history, current_value_totals
    )


def _truncate(text: str, max_len: int) -> str:
    """One-line, length-bounded summary for trace narrative output.
    Newlines collapse to spaces so the rendered trace stays tabular."""
    flat = " ".join(text.split())
    if len(flat) <= max_len:
        return flat
    return flat[: max_len - 1] + "…"


def _kind_counts(signals: dict[str, list[Signal]]) -> dict[str, int]:
    counter: Counter[str] = Counter()
    for sig_list in signals.values():
        for sig in sig_list:
            counter[sig.name] += 1
    return dict(counter)


def _kind_value_totals(signals: dict[str, list[Signal]]) -> dict[str, float]:
    """Sum each signal kind's value across every file.

    Two files with `fan_out=15` and `fan_out=8` produce a fan_out
    total of 23. This is the metric scenario runners need to track —
    instance counts alone (a file either carries fan_out or not)
    cannot reflect "fan_out dropped from 15 to 2".
    """
    totals: dict[str, float] = {}
    for sig_list in signals.values():
        for sig in sig_list:
            totals[sig.name] = totals.get(sig.name, 0.0) + float(sig.value)
    return totals


def _build_context(
    task: str, root: str, scope: list[str] | None, include_snippets: bool
) -> PlanContext:
    py_files = _discover_py_files(root)
    rel_files = [str(Path(f).relative_to(root)) for f in py_files]

    signals: dict[str, list[Signal]] = {}
    signal_layer = SignalLayer()
    for abs_path, rel_path in zip(py_files, rel_files):
        try:
            sigs = signal_layer.extract(abs_path)
            if sigs:
                signals[rel_path] = sigs
        except Exception:
            continue

    edges: list[tuple[str, str]] = []
    for abs_path in py_files:
        try:
            for imp in get_imports(abs_path):
                edges.append((abs_path, imp))
        except Exception:
            continue

    graph = GraphService()
    graph.build(py_files, root)
    has_cycle = graph.has_cycle()

    snippets: dict[str, str] = {}
    if include_snippets:
        in_scope = _scope_filter(py_files, root, scope)
        for abs_path in in_scope[:30]:
            rel = str(Path(abs_path).relative_to(root))
            try:
                snippets[rel] = Path(abs_path).read_text(encoding="utf-8")
            except Exception:
                continue

    return PlanContext(
        task=task,
        root=root,
        scope=scope,
        py_files=rel_files,
        signals=signals,
        graph_edges=edges,
        has_cycle=has_cycle,
        file_snippets=snippets,
    )


def _discover_py_files(root: str) -> list[str]:
    found: list[str] = []
    for dirpath, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if not d.startswith(".") and d != "__pycache__"]
        for f in files:
            if f.endswith(".py"):
                found.append(os.path.join(dirpath, f))
    return sorted(found)


def _scope_filter(
    py_files: list[str], root: str, scope: list[str] | None
) -> list[str]:
    if not scope:
        return py_files
    scope_abs = [Path(root, s).resolve() for s in scope]
    kept: list[str] = []
    for f in py_files:
        fp = Path(f).resolve()
        for s in scope_abs:
            try:
                fp.relative_to(s)
                kept.append(f)
                break
            except ValueError:
                continue
    return kept


def _hash_plan(plan: PatchPlan) -> str:
    data = plan_to_dict(plan)
    data.pop("iteration", None)
    data.pop("parent_id", None)
    blob = json.dumps(data, sort_keys=True).encode("utf-8")
    return hashlib.sha256(blob).hexdigest()


def _regressed(
    before: dict[str, list[Signal]], after: dict[str, list[Signal]]
) -> bool:
    """Did the patch make the codebase worse?

    Uses total signal *cost* (sum of values across files), not
    instance count. The instance-count strategy used previously
    false-positive'd legitimate refactors that produced new files —
    every new file picks up zero-valued signals (fan_out=0,
    max_chain_depth=0) which raised the count without raising any
    actual cost. cost-based comparison answers the question the
    pipeline is actually asking: "did this change degrade the
    codebase's structural quality?".
    """
    return _total_cost(after) > _total_cost(before)


def _total_cost(signals: dict[str, list[Signal]]) -> float:
    """Sum every signal value across every file. New files with
    all-zero signals contribute 0 — by design, so a benign split
    doesn't look like regression."""
    return sum(
        float(sig.value)
        for sig_list in signals.values()
        for sig in sig_list
    )


def _regression_detail(
    before: dict[str, list[Signal]], after: dict[str, list[Signal]]
) -> dict[str, float]:
    """Per-kind cost deltas, restricted to kinds whose cost actually rose.

    Returns the LLM-facing version of "why was this rolled back".
    Empty dict means "no regression". Used to populate
    `PlanContext.previous_regression_detail` so the next planner turn
    can address the specific cost that grew, not just retry blindly.
    """
    before_totals = _kind_value_totals(before)
    after_totals = _kind_value_totals(after)
    detail: dict[str, float] = {}
    for kind in set(before_totals) | set(after_totals):
        delta = after_totals.get(kind, 0.0) - before_totals.get(kind, 0.0)
        if delta > 0:
            detail[kind] = round(delta, 4)
    return detail
