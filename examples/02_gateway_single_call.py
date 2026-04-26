"""
Example 02 — Single-call gateway (Layer A only).

The simplest way to use Aegis: wrap one LLM completion in all the
in-pipeline gates (Ring 0 syntax, ToolCallValidator, IntentClassifier,
IntentBypassDetector, PolicyEngine). The gateway returns text only if
every gate passed; otherwise it retries up to max_retries.

Use this when you have an existing LLM-driven workflow and want to
add Aegis as a thin sanity layer without changing the workflow's loop
structure.

Run from the repo root:
    PYTHONPATH=. python examples/02_gateway_single_call.py
"""
from aegis.agents.gemini import GeminiProvider
from aegis.agents.llm_adapter import LLMGateway


def main() -> None:
    gateway = LLMGateway(llm_provider=GeminiProvider(model_name="gemma-4-31b-it"))

    safe_code = gateway.generate_and_validate(
        prompt="Write a Python function fibonacci(n) returning the n-th Fibonacci number. Code only.",
        max_retries=2,
    )
    print("=== Safe code ===")
    print(safe_code)

    print("\n=== Decision trace (every gate that fired) ===")
    if gateway.last_trace:
        for ev in gateway.last_trace.events:
            print(f"  {ev.layer:14} {ev.decision:8} {ev.reason}")


if __name__ == "__main__":
    main()
