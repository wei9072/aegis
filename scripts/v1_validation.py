"""
V1 validation sweep — drive every scenario × model × N-runs combination
and dump JSON snapshots into tests/scenarios/<name>/runs/.

Aligned with the V1 charter:
  L4 stability   = same model, many runs, characterise distribution
  L5 cross-model = same scenario across model families, see if system
                   absorbs the variance

Default sweep: 4 scenarios × 5 runs × 3 models = 60 runs.

Usage:
    PYTHONPATH=. python scripts/v1_validation.py
    PYTHONPATH=. python scripts/v1_validation.py --runs 3
    PYTHONPATH=. python scripts/v1_validation.py --scenarios syntax_fix
    PYTHONPATH=. python scripts/v1_validation.py --models gemma-4-31b-it

Snapshot filenames are: runs/<timestamp>__<provider>__<safe_model>.json
so multiple runs of the same model don't collide and the aggregator can
group by model deterministically.

Continues past individual run failures — a single network blip or
provider outage shouldn't tank the whole sweep. Final summary lists
which (scenario, model, run) tuples raised, so they can be retried.
"""
from __future__ import annotations

import argparse
import importlib
import sys
import time
import traceback
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from aegis.agents.gemini import GeminiProvider
from aegis.agents.llm_adapter import LLMProvider
from tests.scenarios._runner import dump_run, run_scenario


# (provider_name, model_id) — the L5 model matrix.
# Six families chosen so each cell contributes new cross-model
# evidence rather than duplicating a model already covered by another
# provider (e.g. gpt-oss is on Groq, so we don't fetch it via
# OpenRouter; gemma-4-31b-it is on Gemini, so not via Groq).
DEFAULT_MODELS: list[tuple[str, str]] = [
    ("gemini", "gemma-4-31b-it"),                       # Google Gemma
    ("groq", "llama-3.3-70b-versatile"),                # Meta Llama 3
    ("groq", "openai/gpt-oss-120b"),                    # OpenAI gpt-oss (reasoning)
    ("groq", "qwen/qwen3-32b"),                         # Alibaba Qwen
    ("openrouter", "inclusionai/ling-2.6-1t:free"),     # known-weak control (anchor mismatch)
    # Dropped 2026-04-26: z-ai/glm-4.5-air:free — the V1.5 sweep
    # observed individual generate() calls taking 2,177s (slow stream,
    # not 429), single-handedly stalling the sweep. Add back later if
    # a snappier backend appears or if total_timeout=90 in
    # OpenAIProvider proves enough to keep its cells timely.
]

DEFAULT_SCENARIOS = ["syntax_fix", "fanout_reduce", "lod_refactor", "regression_rollback"]

# Known provider prefixes for the `--models` CLI argument. We can't
# fall back to "contains slash → openrouter" because Groq model ids
# like `openai/gpt-oss-120b` and `meta-llama/llama-4-scout-17b-16e-instruct`
# also carry slashes. Explicit prefix keeps routing unambiguous.
_PROVIDER_PREFIXES = ("gemini:", "openrouter:", "groq:")


@dataclass
class _Failure:
    scenario: str
    provider: str
    model: str
    run_idx: int
    error: str


def _parse_model_arg(arg: str) -> tuple[str, str]:
    """Parse a --models entry into (provider, model_id).

    Forms accepted:
      - `gemini:gemma-4-31b-it`             → ("gemini", "gemma-4-31b-it")
      - `openrouter:inclusionai/ling:free`  → ("openrouter", "inclusionai/ling:free")
      - `groq:llama-3.3-70b-versatile`      → ("groq", "llama-3.3-70b-versatile")
      - `gemma-4-31b-it`                    → ("gemini", "gemma-4-31b-it") [backward-compat]
    """
    for prefix in _PROVIDER_PREFIXES:
        if arg.startswith(prefix):
            return prefix.rstrip(":"), arg[len(prefix):]
    return "gemini", arg


def _build_provider(provider_name: str, model: str) -> LLMProvider:
    if provider_name == "gemini":
        return GeminiProvider(model_name=model)
    if provider_name == "openrouter":
        from aegis.agents.openrouter import OpenRouterProvider
        return OpenRouterProvider(model_name=model)
    if provider_name == "groq":
        from aegis.agents.groq import GroqProvider
        return GroqProvider(model_name=model)
    raise ValueError(f"unknown provider {provider_name!r}")


def _safe_model_slug(model: str) -> str:
    """OpenRouter ids carry '/' and ':' which would escape runs/."""
    return model.replace("/", "_").replace(":", "_")


def _run_one(
    scenario_name: str,
    provider_name: str,
    model: str,
    run_idx: int,
    runs_root: Path,
) -> Optional[_Failure]:
    label = f"{provider_name}/{model}"
    print(f"  [{run_idx + 1}] {scenario_name} × {label} ... ", end="", flush=True)

    try:
        scenario_mod = importlib.import_module(
            f"tests.scenarios.{scenario_name}.scenario"
        )
        scenario = scenario_mod.SCENARIO
        provider = _build_provider(provider_name, model)
        result = run_scenario(scenario, provider, model_label=label)

        # Millisecond precision — second precision dropped 8/60
        # snapshots in the V1 sweep when fast-fail rate-limit runs
        # (~0.1s each) shared a wall-clock second and overwrote.
        ts = time.strftime("%Y%m%dT%H%M%S") + f"{int((time.time() % 1) * 1000):03d}"
        target = runs_root / scenario_name / "runs" / (
            f"{ts}__{provider_name}__{_safe_model_slug(model)}.json"
        )
        dump_run(result, target)

        marker = "✓" if result.pipeline_success else "✗"
        print(
            f"{marker} {result.iterations_run} iter, "
            f"{result.duration_seconds:.1f}s"
        )
        return None
    except Exception as e:
        print(f"RAISED: {type(e).__name__}: {e}")
        return _Failure(
            scenario=scenario_name,
            provider=provider_name,
            model=model,
            run_idx=run_idx,
            error=f"{type(e).__name__}: {e}\n{traceback.format_exc()}",
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--scenarios",
        nargs="+",
        default=DEFAULT_SCENARIOS,
        help="Scenario names to sweep over (default: all four).",
    )
    parser.add_argument(
        "--models",
        nargs="+",
        default=None,
        help="Model ids to sweep. Use `provider:model` form, e.g. "
             "`gemini:gemma-4-31b-it`, `openrouter:inclusionai/ling-2.6-1t:free`, "
             "`groq:llama-3.3-70b-versatile`. Bare model id (no prefix) "
             "defaults to gemini provider for backward compat. "
             "Default sweep: gemini:gemma-4-31b-it, "
             "groq:llama-3.3-70b-versatile, "
             "openrouter:inclusionai/ling-2.6-1t:free.",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=5,
        help="Runs per (scenario × model) cell. Default: 5.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the matrix that would be executed and exit.",
    )
    args = parser.parse_args()

    if args.models is None:
        models = DEFAULT_MODELS
    else:
        models = [_parse_model_arg(m) for m in args.models]

    repo_root = Path(__file__).parent.parent
    runs_root = repo_root / "tests" / "scenarios"

    total = len(args.scenarios) * len(models) * args.runs
    print("V1 validation sweep")
    print(f"  scenarios:    {', '.join(args.scenarios)}")
    print(f"  models:       {', '.join(f'{p}/{m}' for p, m in models)}")
    print(f"  runs/cell:    {args.runs}")
    print(f"  total runs:   {total}")
    if args.dry_run:
        print("(dry-run, exiting)")
        return 0
    print()

    failures: list[_Failure] = []
    started = time.time()
    for scenario_name in args.scenarios:
        for provider_name, model in models:
            print(f"=== {scenario_name} × {provider_name}/{model}")
            for run_idx in range(args.runs):
                fail = _run_one(
                    scenario_name, provider_name, model, run_idx, runs_root
                )
                if fail is not None:
                    failures.append(fail)
            print()

    elapsed = time.time() - started
    succeeded = total - len(failures)
    print(f"Sweep complete in {elapsed:.0f}s.")
    print(f"  succeeded: {succeeded}/{total}")
    if failures:
        print(f"  failed:    {len(failures)}")
        for f in failures:
            print(f"    - {f.scenario} × {f.provider}/{f.model} run #{f.run_idx + 1}")
            for line in f.error.splitlines()[:1]:
                print(f"      {line}")

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
