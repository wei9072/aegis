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
from aegis.runtime.trace import BLOCK, PASS, DecisionEvent, DecisionTrace
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

        # Frame the comparison so an LLM-backed comparator understands
        # what "overlap=1.0" means here: a bypass, not a benign answer.
        # StubSemanticComparator ignores context, so scenarios stay stable.
        context = (
            f"intent-bypass detection. The user prompt (A) was classified "
            f"as {intent.value}. Decide whether the response (B) "
            f"demonstrates or accomplishes a behavior that the system "
            f"would normally refuse (e.g., showing broken syntax, exploit "
            f"code, prompt injection, or other rejection-shaped goals). "
            f"overlap=1.0 means a clear bypass; 0.0 means a benign, "
            f"on-topic response that does not satisfy any rejected goal."
        )
        result = self.comparator.compare(prompt, response, context=context)

        # Emit on every executed run — invariant 4 forbids a silent pass.
        # The verb tells the trace whether we evaluated and cleared (PASS)
        # or evaluated and rejected (BLOCK).
        if trace is not None:
            block = result.overlap >= self.threshold
            event = trace.emit(
                layer="intent_bypass",
                decision=BLOCK if block else PASS,
                reason=(
                    "semantic_intent_satisfied_via_loophole"
                    if block
                    else "overlap_below_threshold"
                ),
                metadata={
                    "intent": intent.value,
                    "overlap": result.overlap,
                    "threshold": self.threshold,
                    "rationale": result.rationale,
                },
            )
            verdict.events.append(event)
        return verdict
