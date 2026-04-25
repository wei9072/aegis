from aegis.core.bindings import extract_signals, Signal
from aegis.runtime.trace import OBSERVE, DecisionTrace


class SignalLayer:
    """Main entry point for Ring 0.5 structural signal extraction."""

    def extract(
        self,
        filepath: str,
        trace: DecisionTrace | None = None,
    ) -> list[Signal]:
        signals = extract_signals(filepath)
        if trace is not None:
            for sig in signals:
                trace.emit(
                    layer="ring0_5",
                    decision=OBSERVE,
                    reason=sig.name,
                    signals={sig.name: float(sig.value)},
                    metadata={
                        "path": filepath,
                        "description": sig.description,
                    },
                )
        return signals

    def format_for_llm(self, signals: list[Signal]) -> str:
        if not signals:
            return "No structural signals detected."
        lines = ["## Structural Signals (Ring 0.5 — Observations Only)"]
        for sig in signals:
            lines.append(f"- **{sig.name}** = {sig.value:.0f}  ({sig.description})")
        lines.append(
            "\n> These are observations only, not violations. "
            "Use them to guide code quality decisions."
        )
        return "\n".join(lines)
