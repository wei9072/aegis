"""
fanout_reduce verifier — did service.py's fan_out actually drop?

The scenario task asks the LLM to remove unused imports so fan_out
goes from 15 → ~2. Verifier passes if the workspace's service.py
fan_out is below the scenario's `fan_out_target_at_most` threshold
(generous: 5). Anything else is INCOMPLETE.
"""
from __future__ import annotations

from pathlib import Path

from aegis.analysis.signals import SignalLayer
from aegis.runtime.task_verifier import VerifierResult


class FanoutReduceVerifier:
    TARGET_AT_MOST = 5  # matches scenario.expectations["fan_out_target_at_most"]

    def __init__(self):
        self._signals = SignalLayer()

    def verify(self, workspace: Path, trace) -> VerifierResult:
        target = workspace / "service.py"
        if not target.exists():
            return VerifierResult(
                passed=False,
                rationale="service.py not found in workspace",
                evidence={"path": str(target)},
            )
        try:
            sigs = self._signals.extract(str(target))
        except Exception as e:
            return VerifierResult(
                passed=False,
                rationale=f"signal extraction failed: {type(e).__name__}: {e}",
                evidence={"extract_error": str(e)},
            )
        fan_out_signals = [s for s in sigs if s.name == "fan_out"]
        if not fan_out_signals:
            # No fan_out signal at all means the file is now trivial
            # (zero imports) — that satisfies "fan_out reduced".
            return VerifierResult(
                passed=True,
                rationale="service.py has no fan_out signal (0 imports)",
                evidence={"fan_out": 0},
            )
        fan_out = max(s.value for s in fan_out_signals)
        passed = fan_out <= self.TARGET_AT_MOST
        return VerifierResult(
            passed=passed,
            rationale=(
                f"service.py fan_out = {fan_out:g} "
                f"({'≤' if passed else '>'} target {self.TARGET_AT_MOST})"
            ),
            evidence={"fan_out": fan_out, "target_at_most": self.TARGET_AT_MOST},
        )
