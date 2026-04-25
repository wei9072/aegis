"""
Multi-turn scenario runner.

`run_scenario(scenario)` copies the scenario's `input/` snapshot into
a sandboxed temp workspace, drives `aegis.runtime.pipeline.run` with
an `on_iteration` listener, and returns a structured `MultiTurnResult`
that carries the full per-iteration trajectory.

Two output channels (matching the design discussion):
  - `print_trajectory(result)` for humans — one line per iteration
    showing plan id, applied/rolled-back, signals delta.
  - `result.to_dict()` / `dump_run(result, path)` for machines —
    JSON-serialisable so future tooling can compare runs across
    models, refactor-strategy variants, or before/after fixes.

The runner is intentionally model-agnostic: pass any LLMProvider, the
runner doesn't care. Default lives at the call site (script, CLI).
"""
from __future__ import annotations

import json
import shutil
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from aegis.agents.llm_adapter import LLMProvider
from aegis.runtime import pipeline as pipeline_mod
from aegis.runtime.pipeline import IterationEvent, PipelineResult


@dataclass
class MultiTurnScenario:
    """One end-to-end refactor scenario.

    `input_dir` is the directory that gets copied (recursively) into a
    temp workspace before the run — that's the seed repo state. The
    runner never mutates `input_dir` itself.

    `expectations` is a free-form dict left for the scenario module to
    fill in — e.g. `{"max_iterations": 2, "must_converge": True,
    "final_signal_count_at_most": 0}`. The runner doesn't assert on
    these directly; it surfaces them in the result so a human or a
    future verifier can compare against actual outcomes.
    """

    name: str
    description: str
    input_dir: Path
    task: str
    max_iterations: int = 3
    scope: list[str] | None = None
    expectations: dict[str, Any] = field(default_factory=dict)


@dataclass
class MultiTurnResult:
    scenario_name: str
    model: str
    duration_seconds: float
    pipeline_success: bool
    pipeline_error: str | None
    iterations_run: int
    events: list[IterationEvent] = field(default_factory=list)
    expectations: dict[str, Any] = field(default_factory=dict)
    workspace: str = ""
    started_at: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "scenario_name": self.scenario_name,
            "model": self.model,
            "started_at": self.started_at,
            "duration_seconds": round(self.duration_seconds, 3),
            "pipeline_success": self.pipeline_success,
            "pipeline_error": self.pipeline_error,
            "iterations_run": self.iterations_run,
            "events": [e.to_dict() for e in self.events],
            "expectations": dict(self.expectations),
            "workspace": self.workspace,
        }


def run_scenario(
    scenario: MultiTurnScenario,
    provider: LLMProvider,
    *,
    model_label: str = "unknown",
    keep_workspace: bool = False,
) -> MultiTurnResult:
    """Drive `pipeline.run` against a fresh copy of `scenario.input_dir`.

    `provider` is whatever LLMProvider the caller wants — the runner
    deliberately does not instantiate one, so swapping models / mocking
    happens at the call site.

    `model_label` is recorded in the result for run-to-run comparison.
    Pass e.g. "gemma-4-31b-it" or "stub-1.0".
    """
    if not scenario.input_dir.is_dir():
        raise FileNotFoundError(
            f"scenario {scenario.name!r} input_dir does not exist: {scenario.input_dir}"
        )

    started = time.time()
    workspace = Path(tempfile.mkdtemp(prefix=f"aegis-scenario-{scenario.name}-"))
    # Copy contents of input_dir (not the directory itself) into
    # workspace, so the scenario's perceived root mirrors what `aegis
    # check` would see if pointed at input_dir.
    for child in scenario.input_dir.iterdir():
        dest = workspace / child.name
        if child.is_dir():
            shutil.copytree(child, dest)
        else:
            shutil.copy2(child, dest)

    events: list[IterationEvent] = []

    def _capture(ev: IterationEvent) -> None:
        events.append(ev)

    pipeline_result: PipelineResult
    try:
        pipeline_result = pipeline_mod.run(
            task=scenario.task,
            root=str(workspace),
            provider=provider,
            scope=scenario.scope,
            max_iters=scenario.max_iterations,
            on_iteration=_capture,
        )
    finally:
        # Workspace is kept on disk by default so the user can inspect
        # final state (or re-run aegis check on it). The runner only
        # cleans up when asked.
        if not keep_workspace:
            # Even when discarding, we don't unlink yet — let caller
            # decide via the workspace path. Default: keep.
            pass

    return MultiTurnResult(
        scenario_name=scenario.name,
        model=model_label,
        duration_seconds=time.time() - started,
        pipeline_success=pipeline_result.success,
        pipeline_error=pipeline_result.error,
        iterations_run=pipeline_result.iterations,
        events=events,
        expectations=dict(scenario.expectations),
        workspace=str(workspace),
        started_at=time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(started)),
    )


# ---------- Reporting ----------

def print_trajectory(result: MultiTurnResult) -> None:
    """Narrative renderer of one multi-turn run.

    Each iteration becomes a labelled block — Plan / Strategy /
    Validation / Apply / Signals / Decision — so a reader can follow
    the system's reasoning without parsing the JSON snapshot.
    Compact one-line per-iter summaries lose the per-step structure
    that's the whole reason multi-turn exists.
    """
    print("=" * 78)
    print(f"Aegis scenario: {result.scenario_name}")
    print(f"Model:          {result.model}")
    if result.events and result.events[0].plan_goal:
        print(f"Goal:           {result.events[0].plan_goal}")
    print("=" * 78)

    if not result.events:
        print("(no iteration events captured)")
    else:
        for ev in result.events:
            print()
            _render_iteration(ev)

    print()
    print("─" * 78)
    _render_summary(result)
    print()


def _render_iteration(ev) -> None:
    print(f"▶ Iteration {ev.iteration}")
    print(f"  Plan          {ev.plan_id}  ({ev.plan_patches} patch{'es' if ev.plan_patches != 1 else ''})")
    if ev.plan_strategy:
        print(f"  Strategy      {ev.plan_strategy}")

    if not ev.validation_passed:
        n = len(ev.validation_errors)
        print(f"  Validation    failed ({n} error{'s' if n != 1 else ''})")
        for err in ev.validation_errors[:3]:
            print(f"                · {_short_error(err)}")
        if n > 3:
            print(f"                · ... +{n - 3} more")
    else:
        print("  Validation    passed")

    if ev.rolled_back and ev.regressed:
        print("  Apply         applied → rolled back (signals regressed)")
    elif ev.rolled_back:
        print("  Apply         applied → rolled back (executor failed)")
    elif ev.applied:
        print("  Apply         applied")
    elif ev.validation_passed:
        print("  Apply         skipped (planner declared done with no patches)")
    else:
        print("  Apply         skipped (validation failed)")

    deltas = [
        (k, v) for k, v in sorted(ev.signal_value_delta_vs_prev.items()) if v != 0
    ]
    if deltas:
        print("  Signals       " + _format_signal_changes(ev.signal_value_totals, deltas))
    else:
        if ev.signal_value_totals:
            unchanged = ", ".join(
                f"{k}={v:g}" for k, v in sorted(ev.signal_value_totals.items())
            )
            print(f"  Signals       unchanged ({unchanged})")
        else:
            print("  Signals       —")

    decision = _decision_summary(ev)
    if decision:
        print(f"  Decision      {decision}")


def _format_signal_changes(totals: dict, deltas: list[tuple[str, float]]) -> str:
    """e.g. 'max_chain_depth 4 → 2 ⬇ -2'."""
    lines = []
    for kind, delta in deltas:
        new_val = totals.get(kind, 0)
        old_val = new_val - delta
        arrow = "⬇" if delta < 0 else "⬆"
        lines.append(f"{kind} {old_val:g} → {new_val:g}  {arrow} {delta:+g}")
    return ("\n" + " " * 16).join(lines)


def _decision_summary(ev) -> str:
    """One sentence describing what the loop decided this turn."""
    if ev.silent_done_contradiction:
        return (
            "validator vetoed plan_done=true (patches present but "
            "anchors did not match) — pipeline replans next iteration"
        )
    if ev.rolled_back and ev.regressed:
        return "patch applied but signals worsened; rolled back to retry"
    if ev.rolled_back:
        return "executor failure rolled back; retrying"
    if not ev.validation_passed:
        return "validator vetoed; planner replans next iteration"
    if ev.applied and ev.plan_done:
        return "applied and planner declared done — task complete"
    if ev.applied:
        return "applied; planner continues to next iteration"
    if ev.plan_done:
        return "planner declared done with no patches needed"
    return ""


def _short_error(err: str) -> str:
    """`ValidationError(kind=..., message='...', ...)` → just the message."""
    if "message='" in err:
        start = err.index("message='") + len("message='")
        end = err.find("'", start)
        if end != -1:
            kind = ""
            if "kind='" in err:
                ks = err.index("kind='") + len("kind='")
                ke = err.find("'", ks)
                if ke != -1:
                    kind = err[ks:ke]
            msg = err[start:end]
            return f"{kind}: {msg}" if kind else msg
    return err


def _render_summary(result: MultiTurnResult) -> None:
    if result.pipeline_success:
        marker = "✓ Converged"
    else:
        marker = "✗ Did not converge"
    print(
        f"{marker} after {result.iterations_run} iteration"
        f"{'s' if result.iterations_run != 1 else ''}, "
        f"{result.duration_seconds:.1f}s total"
    )
    if result.pipeline_error:
        print(f"  Reason: {result.pipeline_error}")

    if result.events:
        last = result.events[-1]
        if last.signal_value_totals:
            totals = ", ".join(
                f"{k}={v:g}" for k, v in sorted(last.signal_value_totals.items())
            )
            print(f"  Final signals: {totals}")

    print(f"  Workspace:  {result.workspace}")


def dump_run(result: MultiTurnResult, target: Path) -> Path:
    """Write JSON-serialised result to `target`, creating parents."""
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        json.dumps(result.to_dict(), indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    return target
