"""
regression_rollback verifier — did the refactor preserve cost AND
actually change the structure?

This scenario is the trickiest of the four because the task ("clearer
separation of concerns") has no narrow signal-based pass condition.
Two-part verdict:

  1. **Non-regression** — sum of signal values across the workspace
     must not exceed the seed's. This is the same invariant the
     pipeline's `_regressed()` rolls back on, applied at task end.

  2. **Structural change** — the file content must actually differ
     from the seed; a no-op "I gave up" is not a solved task even if
     it satisfies non-regression vacuously.

Both must hold. SOLVED iff (cost_after ≤ cost_before) AND
(content_changed). This explicitly does *not* try to verify "concerns
are well-separated" — that's semantic-correctness territory and
belongs to a future LLM-judge layer, not Layer C.
"""
from __future__ import annotations

from pathlib import Path

from aegis.analysis.signals import SignalLayer
from aegis.runtime.task_verifier import VerifierResult


class RegressionRollbackVerifier:
    def __init__(self, seed_dir: Path):
        self._signals = SignalLayer()
        self._seed_dir = seed_dir
        self._seed_cost = self._workspace_cost(seed_dir)
        self._seed_files = self._snapshot_contents(seed_dir)

    def _workspace_cost(self, root: Path) -> float:
        total = 0.0
        for py in root.rglob("*.py"):
            try:
                sigs = self._signals.extract(str(py))
            except Exception:
                continue
            total += sum(float(s.value) for s in sigs)
        return total

    def _snapshot_contents(self, root: Path) -> dict[str, str]:
        return {
            str(p.relative_to(root)): p.read_text(encoding="utf-8", errors="replace")
            for p in root.rglob("*.py")
        }

    def verify(self, workspace: Path, trace) -> VerifierResult:
        try:
            current_cost = self._workspace_cost(workspace)
        except Exception as e:
            return VerifierResult(
                passed=False,
                rationale=f"cost computation failed: {type(e).__name__}: {e}",
                evidence={"error": str(e)},
            )
        current_contents = self._snapshot_contents(workspace)
        content_changed = current_contents != self._seed_files
        cost_ok = current_cost <= self._seed_cost

        if not content_changed:
            return VerifierResult(
                passed=False,
                rationale=(
                    f"workspace identical to seed — refactor never landed "
                    f"(cost {current_cost:g} unchanged from {self._seed_cost:g})"
                ),
                evidence={
                    "cost_before": self._seed_cost,
                    "cost_after": current_cost,
                    "content_changed": False,
                },
            )
        if not cost_ok:
            return VerifierResult(
                passed=False,
                rationale=(
                    f"cost regressed: {current_cost:g} > seed {self._seed_cost:g}"
                ),
                evidence={
                    "cost_before": self._seed_cost,
                    "cost_after": current_cost,
                    "content_changed": True,
                },
            )
        return VerifierResult(
            passed=True,
            rationale=(
                f"cost {current_cost:g} ≤ seed {self._seed_cost:g}, "
                f"workspace structurally changed"
            ),
            evidence={
                "cost_before": self._seed_cost,
                "cost_after": current_cost,
                "content_changed": True,
            },
        )
