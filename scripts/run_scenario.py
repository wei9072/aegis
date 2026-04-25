"""
Run a multi-turn scenario from `tests/scenarios/<name>/` against a
real LLM provider. Prints a human-readable trajectory and writes a
JSON snapshot to `tests/scenarios/<name>/runs/<timestamp>.json`.

Usage:
    PYTHONPATH=. python scripts/run_scenario.py syntax_fix
    PYTHONPATH=. python scripts/run_scenario.py syntax_fix --model gemini-2.5-flash
    PYTHONPATH=. python scripts/run_scenario.py syntax_fix --keep-workspace

This script costs LLM tokens. It is NOT part of `pytest` /
`aegis eval` — those stay deterministic and free.
"""
from __future__ import annotations

import argparse
import importlib
import sys
import time
from pathlib import Path

from aegis.agents.gemini import GeminiProvider
from tests.scenarios._runner import (
    MultiTurnScenario,
    dump_run,
    print_trajectory,
    run_scenario,
)


DEFAULT_MODEL = "gemma-4-31b-it"
SCENARIO_ROOT = Path(__file__).parent.parent / "tests" / "scenarios"


def _list_scenarios() -> list[str]:
    return sorted(
        d.name
        for d in SCENARIO_ROOT.iterdir()
        if d.is_dir() and (d / "scenario.py").exists()
    )


def _load_scenario(name: str) -> MultiTurnScenario:
    module = importlib.import_module(f"tests.scenarios.{name}.scenario")
    if not hasattr(module, "SCENARIO"):
        raise AttributeError(
            f"tests/scenarios/{name}/scenario.py must export a SCENARIO "
            f"variable (a MultiTurnScenario instance)."
        )
    return module.SCENARIO


def main() -> int:
    available = _list_scenarios()
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "scenario",
        choices=available or ["<none-available>"],
        help="Scenario name (directory under tests/scenarios/).",
    )
    parser.add_argument(
        "--model",
        default=DEFAULT_MODEL,
        help=f"Model id passed to GeminiProvider (default: {DEFAULT_MODEL}).",
    )
    parser.add_argument(
        "--keep-workspace",
        action="store_true",
        help="Reserved flag — workspace is currently always retained for inspection.",
    )
    args = parser.parse_args()

    scenario = _load_scenario(args.scenario)
    provider = GeminiProvider(model_name=args.model)
    result = run_scenario(scenario, provider, model_label=args.model)

    print_trajectory(result)

    runs_dir = SCENARIO_ROOT / args.scenario / "runs"
    timestamp = time.strftime("%Y%m%dT%H%M%S")
    target = runs_dir / f"{timestamp}__{args.model}.json"
    dump_run(result, target)
    print(f"\nrun snapshot: {target}")

    return 0 if result.pipeline_success else 1


if __name__ == "__main__":
    sys.exit(main())
