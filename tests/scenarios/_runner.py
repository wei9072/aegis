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
    """Human-readable single-block summary of one run."""
    print("=" * 78)
    print(f"scenario: {result.scenario_name}   model: {result.model}")
    print(
        f"duration: {result.duration_seconds:.2f}s   "
        f"iterations: {result.iterations_run}   "
        f"pipeline_success: {result.pipeline_success}"
    )
    if result.pipeline_error:
        print(f"pipeline_error: {result.pipeline_error}")
    print(f"workspace: {result.workspace}")
    print("-" * 78)

    if not result.events:
        print("(no iteration events captured)")
        return

    for ev in result.events:
        flags = []
        if ev.plan_done:
            flags.append("done")
        if ev.applied:
            flags.append("applied")
        if ev.rolled_back:
            flags.append("rolled_back")
        if ev.regressed:
            flags.append("regressed")
        if not ev.validation_passed:
            flags.append(f"validation_failed({len(ev.validation_errors)})")
        flag_str = " ".join(flags) or "no-op"
        # Show value-deltas (e.g. fan_out:-13) — instance-count deltas
        # alone hide the metric movement we actually care about.
        delta_str = ", ".join(
            f"{k}{v:+g}"
            for k, v in sorted(ev.signal_value_delta_vs_prev.items())
            if v != 0
        ) or "—"
        # Compact totals: fan_out=2, max_chain_depth=1
        totals_str = ", ".join(
            f"{k}={v:g}"
            for k, v in sorted(ev.signal_value_totals.items())
        ) or "—"
        print(
            f"  iter {ev.iteration}  plan={ev.plan_id}  patches={ev.plan_patches}  "
            f"totals[{totals_str}]  Δ={delta_str}  {flag_str}"
        )
        if ev.validation_errors:
            for err in ev.validation_errors[:3]:
                print(f"      - {err}")
            if len(ev.validation_errors) > 3:
                print(f"      ... +{len(ev.validation_errors) - 3} more")
    print("=" * 78)


def dump_run(result: MultiTurnResult, target: Path) -> Path:
    """Write JSON-serialised result to `target`, creating parents."""
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        json.dumps(result.to_dict(), indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    return target
