"""
V1 validation aggregator — read tests/scenarios/*/runs/*.json and emit
markdown tables grouped by (scenario, model) so the V1 charter L4/L5
evidence is readable at a glance.

Two outputs:
  1. Per-scenario detail table — one row per run, columns:
     model, run, iter count, success, decision-pattern path, final cost,
     rolled-back-this-run.
  2. Cross-scenario stability summary — one row per (scenario, model):
     n runs, success rate, iter range, rollback count, distinct pattern
     paths.

The aggregator is forgiving about schema drift — runs from before the
value-totals or observed-patterns fields existed will fall back to
deriving what they can from the per-event flags.

Usage:
    PYTHONPATH=. python scripts/v1_aggregate.py
    PYTHONPATH=. python scripts/v1_aggregate.py > docs/v1_validation.md
    PYTHONPATH=. python scripts/v1_aggregate.py --scenario lod_refactor
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class _RunRecord:
    scenario: str
    model: str
    started_at: str
    duration_seconds: float
    pipeline_success: bool
    iterations_run: int
    pattern_path: list[str]
    rollback_count: int
    final_cost: float | None
    task_pattern: str          # Layer C — TaskPattern.value or "—" for old snapshots
    task_rationale: str        # verifier-supplied one-liner, "" if absent
    snapshot_path: Path


def _derive_pattern_for_event(ev: dict[str, Any]) -> str:
    """Reconstruct a DecisionPattern label from event flags.

    Mirrors derive_pattern() in aegis/runtime/decision_pattern.py — order
    matters; silent_done before validation_veto.
    """
    applied = ev.get("applied", False)
    plan_done = ev.get("plan_done", False)
    plan_patches = ev.get("plan_patches", 0)
    validation_passed = ev.get("validation_passed", False)
    rolled_back = ev.get("rolled_back", False)
    regressed = ev.get("regressed", False)

    if rolled_back and regressed:
        return "regression_rollback"
    if rolled_back:
        return "executor_failure"
    if applied and plan_done:
        return "applied_done"
    if applied:
        return "applied_continuing"
    if plan_done and plan_patches > 0 and validation_passed:
        return "silent_done_veto"
    if plan_done and plan_patches == 0:
        return "noop_done"
    if not validation_passed:
        return "validation_veto"
    return "unknown"


def _final_cost(events: list[dict[str, Any]]) -> float | None:
    """Sum of all signal values from the last event with value totals."""
    for ev in reversed(events):
        totals = ev.get("signal_value_totals")
        if totals:
            return float(sum(totals.values()))
        # Older runs only have signals_by_kind (instance counts, not values)
        kinds = ev.get("signals_by_kind")
        if kinds:
            return float(sum(kinds.values()))
    return None


def _load_runs(runs_root: Path) -> list[_RunRecord]:
    records: list[_RunRecord] = []
    for snap in sorted(runs_root.glob("*/runs/*.json")):
        try:
            data = json.loads(snap.read_text(encoding="utf-8"))
        except Exception as e:
            print(f"# WARN: could not read {snap}: {e}", file=sys.stderr)
            continue

        events = data.get("events", [])
        # Prefer pre-computed observed_patterns when present, else derive.
        pattern_path = data.get("observed_patterns") or [
            _derive_pattern_for_event(ev) for ev in events
        ]
        rollback_count = sum(1 for ev in events if ev.get("rolled_back"))

        # Layer C — task_verdict added in V1.5; older snapshots lack it
        # entirely. Treat absence as "—" rather than NO_VERIFIER, since
        # NO_VERIFIER means the verifier was wired but absent for that
        # scenario, while "—" means the snapshot predates the feature.
        verdict = data.get("task_verdict") or {}
        task_pattern = verdict.get("pattern", "—")
        verifier_result = verdict.get("verifier_result") or {}
        task_rationale = verifier_result.get("rationale", "")

        records.append(_RunRecord(
            scenario=data.get("scenario_name", snap.parent.parent.name),
            model=data.get("model", "unknown"),
            started_at=data.get("started_at", ""),
            duration_seconds=float(data.get("duration_seconds", 0)),
            pipeline_success=bool(data.get("pipeline_success", False)),
            iterations_run=int(data.get("iterations_run", 0)),
            pattern_path=pattern_path,
            rollback_count=rollback_count,
            final_cost=_final_cost(events),
            task_pattern=task_pattern,
            task_rationale=task_rationale,
            snapshot_path=snap,
        ))
    return records


def _normalise_model(model: str) -> str:
    """Strip provider prefix so 'gemini/gemma-4-31b-it' and 'gemma-4-31b-it'
    sort into the same bucket. Older runs stored bare model id, newer
    runs store provider/model."""
    if "/" in model:
        return model.split("/", 1)[1]
    return model


def _format_pattern_path(path: list[str]) -> str:
    if not path:
        return "—"
    if len(path) == 1:
        return path[0]
    return " → ".join(path)


def _print_per_scenario_detail(records: list[_RunRecord]) -> None:
    by_scenario: dict[str, list[_RunRecord]] = defaultdict(list)
    for r in records:
        by_scenario[r.scenario].append(r)

    for scenario in sorted(by_scenario):
        runs = sorted(
            by_scenario[scenario],
            key=lambda r: (_normalise_model(r.model), r.started_at),
        )
        print(f"### {scenario} — per-run detail")
        print()
        print(
            "| model | started | iters | pipeline | pattern path | "
            "final cost | rollbacks | task verdict |"
        )
        print("|---|---|---|---|---|---|---|---|")
        for r in runs:
            ok = "✓" if r.pipeline_success else "✗"
            cost = f"{r.final_cost:g}" if r.final_cost is not None else "—"
            verdict = r.task_pattern
            print(
                f"| `{_normalise_model(r.model)}` | {r.started_at} | "
                f"{r.iterations_run} | {ok} | "
                f"{_format_pattern_path(r.pattern_path)} | "
                f"{cost} | {r.rollback_count} | `{verdict}` |"
            )
        print()


def _print_stability_summary(records: list[_RunRecord]) -> None:
    by_cell: dict[tuple[str, str], list[_RunRecord]] = defaultdict(list)
    for r in records:
        by_cell[(r.scenario, _normalise_model(r.model))].append(r)

    print("### Stability summary — runs grouped by (scenario, model)")
    print()
    print(
        "| scenario | model | n | pipeline ok | **task SOLVED** | "
        "iter min/median/max | rollback runs | distinct paths | mode path |"
    )
    print("|---|---|---|---|---|---|---|---|---|")

    for (scenario, model) in sorted(by_cell):
        runs = by_cell[(scenario, model)]
        n = len(runs)
        ok_count = sum(1 for r in runs if r.pipeline_success)
        # Layer C — task-level success, separate from pipeline_success.
        # Counts only SOLVED; INCOMPLETE / ABANDONED / NO_VERIFIER /
        # VERIFIER_ERROR all count as not-solved.
        solved_count = sum(1 for r in runs if r.task_pattern == "solved")
        verdicts_counter = Counter(r.task_pattern for r in runs)
        # Distinguish "no Layer C evidence at all" (snapshot predates
        # verifier feature, all "—") from "verifier ran but solved=0".
        if all(r.task_pattern == "—" for r in runs):
            verdicts_str = "n/a (pre-verifier)"
        elif solved_count == n or solved_count == 0:
            verdicts_str = f"{solved_count}/{n}"
        else:
            verdicts_str = (
                f"{solved_count}/{n} "
                f"({', '.join(f'{p}={c}' for p, c in verdicts_counter.most_common())})"
            )

        iters = sorted(r.iterations_run for r in runs)
        i_min = iters[0]
        i_max = iters[-1]
        i_med = iters[len(iters) // 2]
        rollback_runs = sum(1 for r in runs if r.rollback_count > 0)

        path_counter: Counter[tuple[str, ...]] = Counter()
        for r in runs:
            path_counter[tuple(r.pattern_path)] += 1
        distinct_paths = len(path_counter)
        mode_path, mode_n = path_counter.most_common(1)[0]
        mode_str = (
            f"{_format_pattern_path(list(mode_path))} ({mode_n}/{n})"
            if mode_path
            else "—"
        )

        print(
            f"| {scenario} | `{model}` | {n} | {ok_count}/{n} | "
            f"**{verdicts_str}** | "
            f"{i_min}/{i_med}/{i_max} | {rollback_runs} | "
            f"{distinct_paths} | {mode_str} |"
        )
    print()


def _print_overview(records: list[_RunRecord]) -> None:
    scenarios = sorted({r.scenario for r in records})
    models = sorted({_normalise_model(r.model) for r in records})
    print(f"## V1 validation evidence — aggregated from `tests/scenarios/*/runs/`")
    print()
    print(f"- Snapshots loaded: **{len(records)}**")
    print(f"- Scenarios:        {', '.join(scenarios)}")
    print(f"- Models:           {', '.join(models)}")
    print()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--scenario",
        default=None,
        help="If set, only include runs from this scenario.",
    )
    parser.add_argument(
        "--no-detail",
        action="store_true",
        help="Skip per-run detail table; only emit stability summary.",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).parent.parent
    runs_root = repo_root / "tests" / "scenarios"
    records = _load_runs(runs_root)
    if args.scenario:
        records = [r for r in records if r.scenario == args.scenario]

    if not records:
        print("No run snapshots found.", file=sys.stderr)
        return 1

    _print_overview(records)
    _print_stability_summary(records)
    if not args.no_detail:
        _print_per_scenario_detail(records)

    return 0


if __name__ == "__main__":
    sys.exit(main())
