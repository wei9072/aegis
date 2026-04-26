"""
Layer C — task outcome verification.

Re-export module. Data types and helpers live in `aegis._core` (Rust,
via aegis-pyshim) since V1.0 of the Rust port:

  - `TaskPattern` enum
  - `VerifierResult`, `TaskVerdict`
  - `derive_task_pattern`, `apply_verifier`

The `TaskVerifier` Protocol stays Python-side because it's a
duck-typed extension point — every per-scenario verifier in
`tests/scenarios/*/verifier.py` matches it structurally. The Rust
`apply_verifier` accepts any object that exposes a
`verify(workspace, trace) -> VerifierResult` method.

**Critical design rules** (still enforced — see
`docs/v1_validation.md#framing`):

  1. The verifier runs after `pipeline.run()`'s loop terminates.
  2. Verifier output goes only on `PipelineResult.task_verdict`. It
     is never copied into `PlanContext`, never propagated to the
     next iteration's prompt, never shown to the LLM.
  3. There is no `IterationEvent.verifier_*` field and no
     DecisionPattern triggered by verifier results.
  4. `TaskPattern` is its own enum, not derived from `DecisionPattern`.

These rules are now structurally enforced in `aegis-decision`:
`TaskVerdict` carries no field a loop could consume, and
`TaskVerifier` (Rust trait) has exactly one method.
"""
from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Protocol

from aegis._core import (
    TaskPattern,
    TaskVerdict,
    VerifierResult,
    apply_verifier,
    derive_task_pattern,
)

if TYPE_CHECKING:
    from aegis.runtime.pipeline import IterationEvent


class TaskVerifier(Protocol):
    """Per-scenario verifier. Inspects the final workspace and
    returns a VerifierResult.

    The trace is passed for diagnostic purposes only — verifiers may
    consult it to write better rationale text (e.g. "rolled back N
    times before stopping") but must not use it to decide pass/fail.
    Pass/fail is purely a function of the workspace's final state.
    """

    def verify(
        self, workspace: Path, trace: list["IterationEvent"]
    ) -> VerifierResult: ...


__all__ = [
    "TaskPattern",
    "TaskVerdict",
    "TaskVerifier",
    "VerifierResult",
    "apply_verifier",
    "derive_task_pattern",
]
