# Aegis

> Aegis ensures bad decisions don't stick.

A system for preventing silent regressions in LLM-driven workflows.

> **If you are an AI coding agent** (Claude Code, Cursor, Aider, etc.)
> helping a user install or integrate Aegis, read
> [`AGENTS.md`](AGENTS.md) instead — it has imperative install
> commands, integration templates you can paste, and the framing
> rules you must respect. The rest of this README is for the human
> evaluating whether to adopt Aegis.

---

## What it is

Aegis is a constraint-based behavior harness for LLM systems.
(In the broad sense: a verification environment, not an agent driver.)

It enforces a local closed loop:
each proposed state transition is validated,
and regressions are rejected.

Instead of optimizing model behavior,
Aegis enforces explicit, verifiable constraints,
ensuring that invalid or regressive states do not persist.

---

## Why it exists

LLM systems fail in three ways that current tooling does not catch:

1. Multi-turn refactors accumulate regressions silently
2. LLM-described actions diverge from actual tool calls
3. Structural rules erode without anyone noticing

Aegis exists to make these failures visible and rejectable.

---

## Core mechanism

Aegis controls state transitions:

```
Sₙ → Sₙ₊₁ is allowed only if all constraints are satisfied
and no regression is detected.

Otherwise, the system rolls back to Sₙ.
```

Cost-aware rollback is the only cross-iteration consistent criterion.
Other checks (validation, policy, structural constraints)
act as local guards, not global direction signals.

---

## Design principles

- Do not teach the model what is good
- Only reject what is verifiably bad
- No automatic learning
- No objective optimization

---

## Guarantees

- Regressions are detected and rolled back when constraints are defined
- Invalid states are blocked at the validation layer
- All decisions are recorded in a machine-readable trace

---

## Non-goals

Aegis is not:

- An AI agent
- An optimizer
- A self-improving system

These are deliberate design choices, not future work.
Aegis will not evolve into any of these.
See [`docs/post_launch_discipline.md`](docs/post_launch_discipline.md)
for the explicit deferral list.

---

## Scope

Aegis enforces correctness within a single execution loop,
but does not adapt behavior across executions.

The loop is local, not global.

---

## Quickstart

The example below runs without any API key — it wraps a stub LLM
in the gateway, so you see the gate vocabulary fire on two known
inputs (one passes, one is rejected).

To wrap your own LLM, replace `StubLLM` with a class that calls
your provider and implements the same `.generate(prompt, tools=None)
-> str` method. That single method **is the integration surface**.

```python
from aegis.agents.llm_adapter import LLMGateway


class StubLLM:
    def __init__(self, canned_response: str):
        self.response = canned_response

    def generate(self, prompt: str, tools=None) -> str:
        return self.response


# CASE 1 — valid Python passes every gate.
gateway = LLMGateway(llm_provider=StubLLM("def add(a, b):\n    return a + b\n"))
print("ALLOWED →", gateway.generate_and_validate("write add()", max_retries=1))
for e in gateway.last_trace.events:
    print(f"  {e.layer:14} {e.decision:8} {e.reason}")

# CASE 2 — invalid syntax → Ring 0 emits BLOCK; gateway raises.
gateway = LLMGateway(llm_provider=StubLLM("def add(a, b returns nothing"))
try:
    gateway.generate_and_validate("write add()", max_retries=1)
except RuntimeError as exc:
    print("BLOCKED →", str(exc)[:80])
    for e in gateway.last_trace.events:
        print(f"  {e.layer:14} {e.decision:8} {e.reason}")
```

Runnable copy: [`examples/00_quickstart.py`](examples/00_quickstart.py).
Other patterns (multi-turn pipeline, custom Layer C verifier,
trace consumption): [`examples/`](examples/).

**Build note.** Aegis ships a Rust extension for fast structural-signal
extraction. V0.x has no `pyproject.toml` yet, so `pip install -e .`
doesn't work directly — instead:

```bash
git clone https://github.com/wei9072/aegis && cd aegis
python -m venv .venv && source .venv/bin/activate
pip install maturin pytest click prompt_toolkit google-genai google-generativeai
cd aegis-core-rs && maturin develop --release && cd ..
python examples/00_quickstart.py
```

(Examples self-bootstrap the import path; no `PYTHONPATH=` prefix
needed until `pip install -e .` becomes the canonical setup.)

The build friction is tracked at
[`docs/launch/issue_rust_build_friction.md`](docs/launch/issue_rust_build_friction.md);
PyPI wheels coming once the friction reports stabilise.

---

## Integrations

You're already using Cursor / Claude Code / Aider / Copilot / your
own agent. Aegis is meant to be a **side-channel enforcement layer**
that doesn't ask you to switch tools.

| Boundary | Path | Status |
| :--- | :--- | :--- |
| Commit | [Git pre-commit hook](docs/integrations/git_pre_commit.md) | ✓ ready (5-line bash) |
| PR / merge | [GitHub Action / CI gate](docs/integrations/github_action.md) | ✓ ready (10-line YAML) |
| Agent decision | [MCP server](docs/integrations/mcp_design.md) | 🟡 design pinned, build pending |

Pick whichever boundary fits your workflow; you can stack them.
Index + per-path detail: [`docs/integrations/`](docs/integrations/).

---

## Status

| Layer | State | Notes |
| :--- | :--- | :--- |
| Execution Engine | ✅ | Pipeline + Executor + cost-aware regression rollback (V1) |
| Policy Enforcement | ✅ | 8 in-pipeline gates: Ring 0/0.5, PolicyEngine, DeliveryRenderer, ToolCallValidator T1+T2, IntentClassifier, IntentBypassDetector |
| Decision Trace | ✅ | `DecisionTrace` + 9 `DecisionPattern` + 5-pattern `TaskVerdict`; cross-model evidence in [`docs/v1_validation.md`](docs/v1_validation.md) |
| Eval Harness | 🟡 | 15 deterministic scenarios + 4 multi-turn scenarios; minimal by design — adaptive eval is post-V2 |
| Feedback Layer | ❌ | Out of scope by design — see [Non-goals](#non-goals) and [Critical Principle](docs/gap3_control_plane.md#critical-principle) |

---

## Philosophy

> If Aegis starts learning automatically,
> it has violated its own design.

---

## License

MIT — see [`LICENSE`](LICENSE).

V0.x — interface is stable, package structure (`pyproject.toml` /
PyPI wheels) is not yet.
