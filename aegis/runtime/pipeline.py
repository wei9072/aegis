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
from aegis.runtime.executor import ExecutionResult, Executor
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

    @property
    def silent_done_contradiction(self) -> bool:
        """The Planner declared done but the patch never made it to disk.
        Pipeline correctly ignored the flag, but a downstream observer
        (CLI / report) wants to surface this loudly."""
        return self.plan_done and not self.applied and self.plan_patches > 0

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
            "silent_done_contradiction": self.silent_done_contradiction,
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


def run(
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

    for i in range(max_iters):
        ctx.previous_plan = last_plan
        ctx.previous_errors = last_errors
        ctx.previous_result = last_result
        ctx.previous_regressed = last_regressed

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
        if last_plan_hash is not None and plan_hash == last_plan_hash and not plan.done:
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
                success=False,
                iterations=i + 1,
                final_plan=plan,
                signals_before=signals_before,
                signals_after=ctx.signals,
                error="planner repeated identical plan (stalemate)",
            )
        last_plan_hash = plan_hash

        if plan.done and not plan.patches:
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
            _emit_iteration(
                on_iteration,
                iteration=i,
                plan=plan,
                plan_id=plan_id,
                validation_errors=[str(e) for e in errors],
                applied=False,
                rolled_back=False,
                regressed=False,
                ctx_signals=ctx.signals,
                prev_kind_counts=prev_kind_counts,
                prev_value_totals=prev_value_totals,
            )
            last_plan, last_errors, last_result, last_regressed = plan, errors, None, False
            continue

        result = executor.apply(plan)
        if not result.success:
            _emit_iteration(
                on_iteration,
                iteration=i,
                plan=plan,
                plan_id=plan_id,
                validation_errors=[],
                applied=False,
                rolled_back=True,  # executor returned failure, state restored
                regressed=False,
                ctx_signals=ctx.signals,
                prev_kind_counts=prev_kind_counts,
                prev_value_totals=prev_value_totals,
            )
            last_plan, last_errors, last_result, last_regressed = plan, [], result, False
            continue

        new_ctx = _build_context(task, root_abs, scope, include_file_snippets)
        if _regressed(ctx.signals, new_ctx.signals):
            executor.rollback_result(result)
            result.rolled_back = True
            _emit_iteration(
                on_iteration,
                iteration=i,
                plan=plan,
                plan_id=plan_id,
                validation_errors=[],
                applied=True,
                rolled_back=True,
                regressed=True,
                ctx_signals=ctx.signals,  # post-rollback, same as before
                prev_kind_counts=prev_kind_counts,
                prev_value_totals=prev_value_totals,
            )
            last_plan, last_errors, last_result, last_regressed = plan, [], result, True
            continue

        ctx = new_ctx
        _emit_iteration(
            on_iteration,
            iteration=i,
            plan=plan,
            plan_id=plan_id,
            validation_errors=[],
            applied=True,
            rolled_back=False,
            regressed=False,
            ctx_signals=ctx.signals,
            prev_kind_counts=prev_kind_counts,
            prev_value_totals=prev_value_totals,
        )
        prev_kind_counts = _kind_counts(ctx.signals)
        prev_value_totals = _kind_value_totals(ctx.signals)
        last_plan, last_errors, last_result, last_regressed = plan, [], result, False

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
) -> None:
    if cb is None:
        return
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
    cb(IterationEvent(
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
    ))


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
    def total(d: dict[str, list[Signal]]) -> int:
        return sum(len(v) for v in d.values())
    return total(after) > total(before)
