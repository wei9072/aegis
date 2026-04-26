"""
Example 01 — Multi-turn refactor pipeline (the primary product surface).

Aegis wraps any LLM provider in a control loop that:
  - lets the planner propose patches
  - validates each patch against structural constraints
  - applies via Executor (the only allowed writer)
  - measures cost-aware regression — rolls back if structure worsened
  - emits one IterationEvent per loop iteration with a named DecisionPattern
  - asks the optional Layer C verifier whether the *task* (not just the loop) is done

Run from the repo root:
    PYTHONPATH=. python examples/01_pipeline_basic.py

Requires GEMINI_API_KEY (or GOOGLE_API_KEY) in the environment.
Substitute with OpenRouterProvider / GroqProvider for cross-family runs.
"""
from pathlib import Path

from aegis.agents.gemini import GeminiProvider
from aegis.runtime import pipeline


def main() -> None:
    workspace = Path(__file__).parent / "_scratch_pipeline_basic"
    workspace.mkdir(exist_ok=True)
    target = workspace / "broken.py"
    # Seed: a Python file with a missing colon syntax error.
    target.write_text("def add(a, b)\n    return a + b\n", encoding="utf-8")

    provider = GeminiProvider(model_name="gemma-4-31b-it")

    result = pipeline.run(
        task="There is a syntax error in broken.py. Fix it minimally.",
        root=str(workspace),
        provider=provider,
        max_iters=3,
    )

    print(f"pipeline_success: {result.success}")
    print(f"iterations:      {result.iterations}")
    if result.error:
        print(f"error:           {result.error}")
    if result.task_verdict:
        print(f"task pattern:    {result.task_verdict.pattern.value}")


if __name__ == "__main__":
    main()
