"""
DeliveryRenderer — turns a PolicyVerdict + raw code into a DeliveryView.

The renderer is intentionally dumb: it has no thresholds, no decision
logic, and no opinion about what counts as a problem. Its single job is
to keep two output channels separate so warnings reach humans without
polluting the LLM-visible context.

`delivery:observe warning_surfaced` is emitted exactly once per render
that produced a banner — eval scenarios assert this presence (or, via a
negative assertion, its absence on clean paths).
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Sequence

from aegis.policy.engine import PolicyVerdict
from aegis.runtime.trace import OBSERVE, DecisionEvent, DecisionTrace


# Stable, human-readable labels for known reason codes. Unknown reasons
# fall back to the raw code so new policy reasons surface immediately
# without a blank banner.
_REASON_LABELS: dict[str, str] = {
    "high_fan_out_advisory": "High fan-out detected",
    "high_fan_out_block": "High fan-out — request blocked",
    "demeter_violation_advisory": "Deep method chain detected",
}


@dataclass
class DeliveryView:
    """Two-channel output. `human` may carry a banner; `llm` never does."""

    human: str
    llm: str
    surfaced: bool


class DeliveryRenderer:
    def render(
        self,
        code: str,
        verdict: PolicyVerdict,
        trace: DecisionTrace | None = None,
    ) -> DeliveryView:
        warnings = verdict.warnings()
        if not warnings:
            return DeliveryView(human=code, llm=code, surfaced=False)

        banner = self._format_banner(warnings)
        human = f"{banner}\n\n---\n\n{code}"
        # LLM-bound channel must NOT contain the banner. Recursive
        # pollution of context is the whole reason this layer exists.
        llm = code

        if trace is not None:
            trace.emit(
                layer="delivery",
                decision=OBSERVE,
                reason="warning_surfaced",
                metadata={
                    "channel": "banner",
                    "before_code": True,
                    "warnings": [w.reason for w in warnings],
                },
            )
        return DeliveryView(human=human, llm=llm, surfaced=True)

    @staticmethod
    def _format_banner(warnings: Sequence[DecisionEvent]) -> str:
        lines: list[str] = []
        for ev in warnings:
            label = _REASON_LABELS.get(ev.reason, ev.reason)
            signal = ev.metadata.get("signal")
            value = ev.metadata.get("value")
            threshold = ev.metadata.get("threshold")
            if signal is not None and value is not None and threshold is not None:
                lines.append(
                    f"⚠️  Warning: {label} ({signal}={value:g}, threshold={threshold:g})"
                )
            else:
                lines.append(f"⚠️  Warning: {label}")
        return "\n".join(lines)
