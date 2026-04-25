"""
Evaluation harness primitives.

A `Scenario` declares:
  - a prompt the gateway will receive
  - a sequence of canned LLM responses (deterministic, no API calls)
  - an optional explicit `tools` argument
  - the ordered subsequence of trace events the system MUST produce

The runner builds a fake provider, drives the real LLMGateway end-to-end,
and asserts that every expected event appears in `gateway.last_trace` in
order. Extra events between matches are tolerated — this lets new
observability layers be introduced without invalidating old scenarios.

Mismatches are returned as structured strings; nothing is logged or
printed at this layer so the harness is usable from both pytest and the
`aegis eval` CLI.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Iterable

from aegis.agents.llm_adapter import LLMGateway
from aegis.intent.bypass import IntentBypassDetector
from aegis.runtime.trace import DecisionEvent
from aegis.semantic.comparator import SemanticComparator


# ---------- Scenario LLM stub ----------

class _ScenarioProvider:
    """Deterministic LLM stand-in for scenario replay.

    Records the resolved tool surface (per the new dynamic-tool contract)
    so `provider:tool_surface` trace events carry meaningful metadata
    even without a real provider's default-resolution logic.
    """
    def __init__(self, responses: list[str]) -> None:
        self._responses = list(responses)
        self._cursor = 0
        self.last_used_tools: tuple = ()

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        self.last_used_tools = tuple(tools) if tools is not None else ()
        if self._cursor >= len(self._responses):
            raise RuntimeError(
                "Scenario ran out of canned LLM responses — extend "
                "`llm_responses` or reduce `max_retries`."
            )
        resp = self._responses[self._cursor]
        self._cursor += 1
        return resp


# ---------- Expectation primitives ----------

@dataclass
class ExpectedEvent:
    """A trace event matcher.

    `reason` and `metadata_includes` are optional — `None` / `{}` mean
    "don't constrain this dimension". `metadata_includes` performs an
    exact-equality lookup for each key, so partial structural matches
    on metadata are explicit and readable.
    """
    layer: str
    decision: str
    reason: str | None = None
    metadata_includes: dict[str, Any] = field(default_factory=dict)

    def matches(self, event: DecisionEvent) -> bool:
        if event.layer != self.layer:
            return False
        if event.decision != self.decision:
            return False
        if self.reason is not None and event.reason != self.reason:
            return False
        for key, expected in self.metadata_includes.items():
            if event.metadata.get(key) != expected:
                return False
        return True

    def describe(self) -> str:
        parts = [f"layer={self.layer}", f"decision={self.decision}"]
        if self.reason is not None:
            parts.append(f"reason={self.reason}")
        if self.metadata_includes:
            parts.append(f"metadata~={self.metadata_includes}")
        return " ".join(parts)


# ---------- Scenario + result ----------

@dataclass
class Scenario:
    name: str
    description: str
    prompt: str
    llm_responses: list[str]
    expected_events: list[ExpectedEvent] = field(default_factory=list)
    tools: tuple | None = None
    max_retries: int = 3
    expects_raise: bool = False
    note: str = ""
    # Optional Phase-3 dependency. When set, the harness wires an
    # IntentBypassDetector into the gateway with this comparator. When
    # None, the gateway has no bypass detector at all and Phase-3
    # events are absent — exactly the situation today's scenarios
    # 01-08, 11, 12 want.
    intent_bypass_comparator: SemanticComparator | None = None
    intent_bypass_threshold: float = 0.7

    def run(self) -> ScenarioResult:
        provider = _ScenarioProvider(self.llm_responses)
        bypass = (
            IntentBypassDetector(
                comparator=self.intent_bypass_comparator,
                threshold=self.intent_bypass_threshold,
            )
            if self.intent_bypass_comparator is not None
            else None
        )
        gateway = LLMGateway(llm_provider=provider, intent_bypass=bypass)
        raised: str | None = None
        try:
            gateway.generate_and_validate(
                self.prompt,
                max_retries=self.max_retries,
                tools=self.tools,
            )
        except Exception as e:
            raised = f"{type(e).__name__}: {e}"

        actual_events = list(gateway.last_trace.events) if gateway.last_trace else []
        mismatches = self._diff(actual_events)

        if self.expects_raise and raised is None:
            mismatches.append("scenario expected gateway to raise, but it returned normally")
        if not self.expects_raise and raised is not None:
            mismatches.append(f"scenario did NOT expect a raise, but got: {raised}")

        return ScenarioResult(
            name=self.name,
            passed=not mismatches,
            actual_events=actual_events,
            mismatches=mismatches,
            raised=raised,
        )

    def _diff(self, actual: list[DecisionEvent]) -> list[str]:
        problems: list[str] = []
        cursor = 0
        for expected in self.expected_events:
            found_at = -1
            for i in range(cursor, len(actual)):
                if expected.matches(actual[i]):
                    found_at = i
                    break
            if found_at == -1:
                problems.append(f"missing expected event: {expected.describe()}")
            else:
                cursor = found_at + 1
        return problems


@dataclass
class ScenarioResult:
    name: str
    passed: bool
    actual_events: list[DecisionEvent]
    mismatches: list[str] = field(default_factory=list)
    raised: str | None = None


# ---------- Runner + formatter ----------

def run_all(scenarios: Iterable[Scenario]) -> list[ScenarioResult]:
    return [s.run() for s in scenarios]


def format_results(
    results: list[ScenarioResult],
    *,
    verbose: bool = False,
) -> str:
    lines: list[str] = []
    passed = sum(1 for r in results if r.passed)
    total = len(results)
    for r in results:
        marker = "PASS" if r.passed else "FAIL"
        lines.append(f"[{marker}] {r.name}")
        if r.mismatches:
            for m in r.mismatches:
                lines.append(f"    - {m}")
        if verbose and r.raised:
            lines.append(f"    raised: {r.raised}")
        if verbose and r.passed:
            lines.append(f"    events: {len(r.actual_events)}")
    lines.append("")
    lines.append(f"{passed}/{total} scenarios passed")
    return "\n".join(lines)
