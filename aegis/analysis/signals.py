from aegis.core.bindings import extract_signals, Signal


class SignalLayer:
    """Main entry point for Ring 0.5 structural signal extraction."""

    def extract(self, filepath: str) -> list[Signal]:
        return extract_signals(filepath)

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
