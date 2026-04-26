"""
lod_refactor verifier — did orders.py's max_chain_depth drop to ≤ 2?

The scenario forbids new files (`orders.py` is the only file that may
exist), so the verifier only needs to inspect that one file. Passes
if max_chain_depth ≤ 2, matching the scenario's
`max_chain_depth_target_at_most` expectation.
"""
from __future__ import annotations

from pathlib import Path

from aegis.analysis.signals import SignalLayer
from aegis.runtime.task_verifier import VerifierResult


class LodRefactorVerifier:
    TARGET_AT_MOST = 2  # matches scenario.expectations["max_chain_depth_target_at_most"]

    def __init__(self):
        self._signals = SignalLayer()

    def verify(self, workspace: Path, trace) -> VerifierResult:
        target = workspace / "orders.py"
        if not target.exists():
            return VerifierResult(
                passed=False,
                rationale="orders.py not found in workspace",
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
        chain_signals = [s for s in sigs if s.name == "max_chain_depth"]
        if not chain_signals:
            # No chain depth signal = no method chains found, which
            # satisfies the LoD constraint trivially.
            return VerifierResult(
                passed=True,
                rationale="orders.py has no max_chain_depth signal (no chained calls)",
                evidence={"max_chain_depth": 0},
            )
        max_depth = max(s.value for s in chain_signals)
        passed = max_depth <= self.TARGET_AT_MOST
        return VerifierResult(
            passed=passed,
            rationale=(
                f"orders.py max_chain_depth = {max_depth:g} "
                f"({'≤' if passed else '>'} target {self.TARGET_AT_MOST})"
            ),
            evidence={"max_chain_depth": max_depth, "target_at_most": self.TARGET_AT_MOST},
        )
