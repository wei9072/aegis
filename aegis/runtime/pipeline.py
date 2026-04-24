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
from dataclasses import dataclass, field
from pathlib import Path

from aegis.agents.llm_adapter import LLMProvider
from aegis.agents.planner import LLMPlanner, PlanContext
from aegis.analysis.signals import SignalLayer
from aegis.core.bindings import Signal, get_imports
from aegis.graph.service import GraphService
from aegis.ir.patch import PatchPlan, plan_to_dict
from aegis.runtime.executor import ExecutionResult, Executor
from aegis.runtime.validator import PlanValidator, ValidationError


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
) -> PipelineResult:
    root_abs = str(Path(root).resolve())
    planner = LLMPlanner(provider)
    validator = PlanValidator(root_abs, scope=scope)
    executor = Executor(root_abs)

    ctx = _build_context(task, root_abs, scope, include_file_snippets)
    signals_before = ctx.signals

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
        if last_plan_hash is not None and plan_hash == last_plan_hash and not plan.done:
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
            return PipelineResult(
                success=True,
                iterations=i + 1,
                final_plan=plan,
                signals_before=signals_before,
                signals_after=ctx.signals,
            )

        errors = validator.validate(plan)
        if errors:
            last_plan, last_errors, last_result, last_regressed = plan, errors, None, False
            continue

        result = executor.apply(plan)
        if not result.success:
            last_plan, last_errors, last_result, last_regressed = plan, [], result, False
            continue

        new_ctx = _build_context(task, root_abs, scope, include_file_snippets)
        if _regressed(ctx.signals, new_ctx.signals):
            executor.rollback_result(result)
            result.rolled_back = True
            last_plan, last_errors, last_result, last_regressed = plan, [], result, True
            continue

        ctx = new_ctx
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
