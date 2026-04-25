"""
PolicyEngine — deterministic rule evaluation over DecisionTrace.

The engine reads observations already emitted by upstream gates (today:
Ring 0.5 signals like fan_out and max_chain_depth) and decides whether
the request should be escalated to `warn` or `block`. It performs no
I/O, no LLM calls, and mutates only the trace it is given.

Rules are data, not code: adding a new threshold should not require
touching this module. The default rule set targets Phase 1 — fan_out
and max_chain_depth advisories — and is intentionally conservative.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Iterable

from aegis.runtime.trace import BLOCK, WARN, DecisionEvent, DecisionTrace


@dataclass(frozen=True)
class SignalRule:
    """Fires when `signals[signal_name] >= threshold` is observed.

    `decision` must be one of `WARN` / `BLOCK`. `reason` is the stable
    identifier eval scenarios assert against.
    """

    signal_name: str
    threshold: float
    decision: str
    reason: str


# Order matters within the same signal: stricter thresholds first so the
# stronger verdict wins for one signal in one evaluation pass.
DEFAULT_RULES: tuple[SignalRule, ...] = (
    SignalRule("fan_out", 20.0, BLOCK, "high_fan_out_block"),
    SignalRule("fan_out", 10.0, WARN, "high_fan_out_advisory"),
    SignalRule("max_chain_depth", 5.0, WARN, "demeter_violation_advisory"),
)


@dataclass
class PolicyVerdict:
    """Concrete events the engine appended to the trace this pass.

    Callers (delivery renderer, gateway) read `events` instead of
    re-scanning the whole trace, so the policy/delivery boundary stays
    explicit and ordering remains insensitive to other layers' output.
    """

    events: list[DecisionEvent] = field(default_factory=list)

    def has_block(self) -> bool:
        return any(e.decision == BLOCK for e in self.events)

    def warnings(self) -> list[DecisionEvent]:
        return [e for e in self.events if e.decision == WARN]


class PolicyEngine:
    def __init__(self, rules: Iterable[SignalRule] = DEFAULT_RULES) -> None:
        self._rules: tuple[SignalRule, ...] = tuple(rules)

    @property
    def rules(self) -> tuple[SignalRule, ...]:
        return self._rules

    def evaluate(self, trace: DecisionTrace) -> PolicyVerdict:
        """Apply rules to ring0_5 observations in `trace`.

        For each distinct signal at most one rule fires (the strictest
        matching one). This dedupes so a single fan_out=25 produces
        exactly one `policy:block`, not also a `policy:warn`.
        """
        observed = self._collect_signal_values(trace)
        verdict = PolicyVerdict()
        decided: set[str] = set()
        for rule in self._rules:
            if rule.signal_name in decided:
                continue
            value = observed.get(rule.signal_name)
            if value is None or value < rule.threshold:
                continue
            event = trace.emit(
                layer="policy",
                decision=rule.decision,
                reason=rule.reason,
                signals={rule.signal_name: value},
                metadata={
                    "signal": rule.signal_name,
                    "value": value,
                    "threshold": rule.threshold,
                },
            )
            verdict.events.append(event)
            decided.add(rule.signal_name)
        return verdict

    @staticmethod
    def _collect_signal_values(trace: DecisionTrace) -> dict[str, float]:
        out: dict[str, float] = {}
        for ev in trace.by_layer("ring0_5"):
            for name, value in ev.signals.items():
                # Multiple ring0_5 emissions for the same signal are rare
                # but possible across retries; the strictest observation wins.
                if name not in out or value > out[name]:
                    out[name] = float(value)
        return out
