"""
IntentBypassDetector — post-response semantic gate.

Catches the shape from scenario 09: the prompt asks for something the
system would normally refuse (broken syntax, an exploit, a prompt
injection demo); the response is technically valid but semantically
satisfies that rejection-shaped target via a loophole (hiding `def
bad(` inside a string literal, etc.).

This is the most expensive layer — one extra semantic comparison per
turn — so it only runs when the IntentClassifier already flagged the
prompt as TEACHING or ADVERSARIAL. NORMAL_DEV bypass is presumed
unlikely enough that the cost is not worth it.

Tier-2 ToolCallValidator (Phase 3 follow-up) will reuse the same
SemanticComparator instance: one engine, two callers, one set of
calibration knobs.
"""
from __future__ import annotations

from dataclasses import dataclass, field

from aegis.intent.classifier import Intent
from aegis.runtime.trace import BLOCK, DecisionEvent, DecisionTrace
from aegis.semantic.comparator import SemanticComparator


_GATING_INTENTS: frozenset[Intent] = frozenset({Intent.TEACHING, Intent.ADVERSARIAL})


@dataclass
class BypassVerdict:
    events: list[DecisionEvent] = field(default_factory=list)

    def has_block(self) -> bool:
        return any(e.decision == BLOCK for e in self.events)


class IntentBypassDetector:
    def __init__(
        self,
        comparator: SemanticComparator,
        *,
        threshold: float = 0.7,
    ) -> None:
        self.comparator = comparator
        self.threshold = threshold

    def detect(
        self,
        prompt: str,
        response: str,
        intent: Intent,
        trace: DecisionTrace | None = None,
    ) -> BypassVerdict:
        verdict = BypassVerdict()
        if intent not in _GATING_INTENTS:
            return verdict

        result = self.comparator.compare(prompt, response, context="intent_bypass")
        if result.overlap < self.threshold:
            return verdict

        if trace is not None:
            event = trace.emit(
                layer="intent_bypass",
                decision=BLOCK,
                reason="semantic_intent_satisfied_via_loophole",
                metadata={
                    "intent": intent.value,
                    "overlap": result.overlap,
                    "threshold": self.threshold,
                    "rationale": result.rationale,
                },
            )
            verdict.events.append(event)
        return verdict
