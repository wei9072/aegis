"""
DecisionTrace: structured record of every decision made during a single request.

Each gate (Ring 0, Ring 0.5, ToolCallValidator, policy, intent) emits a
DecisionEvent into a shared DecisionTrace. The trace becomes the primary
artifact verified by the eval harness — assertions are made on the event
sequence, not on the final output text.

Design rules:
  - Events are append-only, totally ordered by emission.
  - Layer names are open strings ("ring0", "ring0_5", "toolcall", "policy",
    "intent", "gateway"); decision verbs are constrained to the four below.
  - `reason` is a short stable identifier ("syntax_invalid", "circular_dependency").
    `metadata` carries free-form structured payload for the layer that emitted.
  - This module is pure data: no I/O, no LLM calls, no logging side-effects.
"""
from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any


# Decision verbs.
PASS = "pass"        # gate evaluated, no issue found
BLOCK = "block"      # gate decided to halt the request
WARN = "warn"        # gate flagged an issue but did not block
OBSERVE = "observe"  # gate recorded a measurement (Ring 0.5 signals, lifecycle)


@dataclass
class DecisionEvent:
    layer: str
    decision: str
    reason: str = ""
    signals: dict[str, float] = field(default_factory=dict)
    metadata: dict[str, Any] = field(default_factory=dict)
    timestamp: float = field(default_factory=time.time)


@dataclass
class DecisionTrace:
    events: list[DecisionEvent] = field(default_factory=list)

    def emit(
        self,
        layer: str,
        decision: str,
        reason: str = "",
        signals: dict[str, float] | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> DecisionEvent:
        event = DecisionEvent(
            layer=layer,
            decision=decision,
            reason=reason,
            signals=signals or {},
            metadata=metadata or {},
        )
        self.events.append(event)
        return event

    def by_layer(self, layer: str) -> list[DecisionEvent]:
        return [e for e in self.events if e.layer == layer]

    def by_decision(self, decision: str) -> list[DecisionEvent]:
        return [e for e in self.events if e.decision == decision]

    def has_block(self) -> bool:
        return any(e.decision == BLOCK for e in self.events)

    def reasons(self) -> list[str]:
        return [e.reason for e in self.events]

    def to_list(self) -> list[dict[str, Any]]:
        return [
            {
                "layer": e.layer,
                "decision": e.decision,
                "reason": e.reason,
                "signals": dict(e.signals),
                "metadata": dict(e.metadata),
                "timestamp": e.timestamp,
            }
            for e in self.events
        ]
