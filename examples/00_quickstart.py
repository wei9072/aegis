"""
Aegis 30-second quickstart — runs with no API key, no network.

Wraps a fake LLM in the gateway and shows two outcomes:
  CASE 1 — clean code passes every gate (Ring 0 syntax / signals /
            intent / policy / tool-call / delivery)
  CASE 2 — invalid syntax → Ring 0 blocks → gateway raises after
            retries are exhausted

To wrap your OWN LLM, replace `StubLLM` with a class that calls
your provider (OpenAI, Anthropic, OpenRouter, vLLM, anything) and
implements the same `.generate(prompt, tools=None) -> str` method.
That's the entire integration surface — see examples/02 for a real
provider.

Run from the repo root:
    python examples/00_quickstart.py

(Self-bootstraps the import path — no PYTHONPATH= prefix needed
until pyproject.toml lands and `pip install -e .` becomes the
canonical setup.)
"""
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from aegis.agents.llm_adapter import LLMGateway  # noqa: E402


class StubLLM:
    """Minimal LLMProvider — implements the one-method Protocol that
    Aegis recognises. Wrap your own LLM the same way."""
    def __init__(self, canned_response: str):
        self.response = canned_response

    def generate(self, prompt: str, tools=None) -> str:
        return self.response


def show(label: str, gateway: LLMGateway) -> None:
    print(f"=== {label} ===")
    print("trace:")
    for e in (gateway.last_trace.events if gateway.last_trace else []):
        print(f"  {e.layer:14} {e.decision:8} {e.reason}")
    print()


# CASE 1 — valid Python passes every gate, gateway returns the text.
gateway = LLMGateway(llm_provider=StubLLM("def add(a, b):\n    return a + b\n"))
result = gateway.generate_and_validate("write add()", max_retries=1)
print(f"ALLOWED → {result.strip()!r}")
show("CASE 1 trace", gateway)


# CASE 2 — invalid syntax → Ring 0 emits BLOCK; gateway retries 0 times
# (so it raises immediately on the first block) and we see which gate fired.
gateway = LLMGateway(llm_provider=StubLLM("def add(a, b returns nothing"))
try:
    gateway.generate_and_validate("write add()", max_retries=1)
except Exception as exc:
    print(f"BLOCKED → {type(exc).__name__}: {str(exc)[:80]}")
    show("CASE 2 trace", gateway)
