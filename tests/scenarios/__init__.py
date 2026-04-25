"""
Multi-turn scenario fixtures + runner.

Distinct from `aegis/eval/scenarios.py`: those are single-turn,
deterministic, fake-provider scenarios that run as part of `pytest`
and `aegis eval`. The ones here are multi-turn, hit a real LLM, cost
tokens, and are NOT auto-collected by pytest. Run them through
`tests.scenarios._runner.run_scenario(...)` or the top-level
script (added separately).

Why both exist: deterministic scenarios verify that decision-trace
shapes do not regress (CI-grade). Multi-turn scenarios verify that
the pipeline's iteration loop converges on real refactor problems
(product-grade). Different audiences, different cadences.
"""
