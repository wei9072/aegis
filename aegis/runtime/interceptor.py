"""
Write interceptor: called before an LLM write is committed.
Runs Ring 0 enforcement; passes signals to LLM for Ring 1 decision.
"""
from aegis.enforcement.validator import Ring0Enforcer
from aegis.analysis.signals import SignalLayer


class WriteInterceptor:
    def __init__(self) -> None:
        self._enforcer = Ring0Enforcer()
        self._signal_layer = SignalLayer()

    def intercept(self, filepath: str) -> tuple[list[str], list]:
        """
        Returns (violations, signals).
        violations non-empty → BLOCK.
        signals always returned for LLM context.
        """
        violations = self._enforcer.check_file(filepath)
        signals = self._signal_layer.extract(filepath) if not violations else []
        return violations, signals
