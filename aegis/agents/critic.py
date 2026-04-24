"""
Critic: converts Ring 0.5 signals into structured LLM feedback.
Takes signals from analysis/ and produces a prompt fragment for Ring 1.
"""
from aegis.analysis.signals import SignalLayer
from aegis.core.bindings import Signal


class Critic:
    """Translates structural signals into actionable LLM guidance."""

    def __init__(self) -> None:
        self._signal_layer = SignalLayer()

    def critique(self, signals: list[Signal]) -> str:
        return self._signal_layer.format_for_llm(signals)
