"""
Layer C — task outcome verification.

Aegis V1 Layer A (per-gate trace events) + Layer B (per-iteration
DecisionPattern) judge **code-state transitions** — did this attempt
make the structure worse? They do not judge **task outcome** — was
the original task actually solved?

This module is the bridge to task-level truth. A `TaskVerifier`
inspects the final workspace and produces a `TaskVerdict` that
reports `SOLVED / INCOMPLETE / ABANDONED / NO_VERIFIER /
VERIFIER_ERROR`.

**Critical design rules** (do not weaken without explicit alignment):

1. The verifier runs **after** `pipeline.run()`'s loop terminates,
   not inside it. There is no per-iteration verifier hook.
2. Verifier output is recorded only on `PipelineResult.task_verdict`.
   It is **never** copied into `PlanContext`, never propagated to
   the next iteration's prompt, never shown to the LLM. If a future
   caller wants to retry on `INCOMPLETE`, that retry is the *caller's*
   responsibility — Aegis does not loop on verifier failure.
3. There is no `IterationEvent.verifier_*` field and no DecisionPattern
   triggered by verifier results. Layer B remains untouched.
4. `TaskPattern` is its own enum, not derived from `DecisionPattern`.

Why these rules: feeding verifier results back into Layer B turns
Aegis from a "decision system that rejects degradation" into a
"goal-seeker that keeps trying until the verifier is satisfied" —
two structurally different things. The latter would justify violating
internal invariants in pursuit of an external goal, which is the
opposite of what Aegis exists for. See
`docs/v1_validation.md#framing` and the "negative-space rejection
harness" memory entry for the full framing.

This module is pure data + Protocol; the actual verifier
implementations live alongside their scenarios (e.g.
`tests/scenarios/syntax_fix/verifier.py`).
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import TYPE_CHECKING, Any, Protocol

if TYPE_CHECKING:
    from aegis.runtime.pipeline import IterationEvent


class TaskPattern(str, Enum):
    """Named outcomes at the task level.

    Five patterns. Names are stable; rename = breaking change for
    downstream tooling (aggregator, scenario reports, eval harness).

    The five exhaust the cases where a verifier is or isn't present
    and whether the pipeline declared done:

      | pipeline_done | verifier         | pattern         |
      |---------------|------------------|-----------------|
      | (any)         | passed           | SOLVED          |
      | True          | failed           | INCOMPLETE      |
      | False         | failed           | ABANDONED       |
      | (any)         | not provided     | NO_VERIFIER     |
      | (any)         | raised exception | VERIFIER_ERROR  |
    """

    SOLVED = "solved"               # verifier said yes, regardless of pipeline state
    INCOMPLETE = "incomplete"       # planner claimed done, but verifier said no — LLM lied
    ABANDONED = "abandoned"         # planner gave up (max iters / stalemate), verifier says no
    NO_VERIFIER = "no_verifier"     # scenario didn't define one
    VERIFIER_ERROR = "verifier_error"  # verifier itself raised


@dataclass(frozen=True)
class VerifierResult:
    """What a TaskVerifier returns after inspecting the workspace.

    `passed` is the only field that drives TaskPattern derivation.
    `rationale` is a human-readable one-liner shown in trajectory
    output. `evidence` is verifier-specific structured data
    (e.g. `{"ast_parsed": True, "max_chain_depth": 1}`) that goes
    into the JSON snapshot for later inspection.
    """

    passed: bool
    rationale: str = ""
    evidence: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class TaskVerdict:
    """Layer C output. Attached to PipelineResult after the loop ends.

    `verifier_result` is None iff `pattern in {NO_VERIFIER,
    VERIFIER_ERROR-without-VerifierResult}`; otherwise it carries the
    raw verifier evidence.
    """

    pattern: TaskPattern
    verifier_result: VerifierResult | None
    pipeline_done: bool
    iterations_run: int
    error: str = ""  # populated only when pattern == VERIFIER_ERROR

    def to_dict(self) -> dict[str, Any]:
        return {
            "pattern": self.pattern.value,
            "pipeline_done": self.pipeline_done,
            "iterations_run": self.iterations_run,
            "error": self.error,
            "verifier_result": (
                None if self.verifier_result is None
                else {
                    "passed": self.verifier_result.passed,
                    "rationale": self.verifier_result.rationale,
                    "evidence": dict(self.verifier_result.evidence),
                }
            ),
        }


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


def derive_task_pattern(
    *,
    verifier_present: bool,
    verifier_passed: bool | None,
    verifier_raised: bool,
    pipeline_done: bool,
) -> TaskPattern:
    """Map (verifier outcome, pipeline state) → TaskPattern.

    Single source of truth for the TaskPattern derivation table in
    the TaskPattern docstring above. Exhaustive: every combination
    of inputs maps to exactly one pattern.
    """
    if not verifier_present:
        return TaskPattern.NO_VERIFIER
    if verifier_raised:
        return TaskPattern.VERIFIER_ERROR
    # verifier ran, produced a verdict
    if verifier_passed:
        return TaskPattern.SOLVED
    # verifier said failed
    return TaskPattern.INCOMPLETE if pipeline_done else TaskPattern.ABANDONED


def apply_verifier(
    verifier: TaskVerifier | None,
    workspace: Path,
    trace: list["IterationEvent"],
    *,
    pipeline_done: bool,
    iterations_run: int,
) -> TaskVerdict:
    """Run the verifier (if any) and wrap the outcome into a TaskVerdict.

    Always returns a TaskVerdict — never raises, never returns None.
    Verifier exceptions are caught and surfaced as VERIFIER_ERROR with
    the exception message in `error`, so a buggy verifier doesn't
    crash the pipeline post-run.
    """
    if verifier is None:
        return TaskVerdict(
            pattern=TaskPattern.NO_VERIFIER,
            verifier_result=None,
            pipeline_done=pipeline_done,
            iterations_run=iterations_run,
        )
    try:
        result = verifier.verify(workspace, trace)
    except Exception as e:
        return TaskVerdict(
            pattern=TaskPattern.VERIFIER_ERROR,
            verifier_result=None,
            pipeline_done=pipeline_done,
            iterations_run=iterations_run,
            error=f"{type(e).__name__}: {e}",
        )
    pattern = derive_task_pattern(
        verifier_present=True,
        verifier_passed=result.passed,
        verifier_raised=False,
        pipeline_done=pipeline_done,
    )
    return TaskVerdict(
        pattern=pattern,
        verifier_result=result,
        pipeline_done=pipeline_done,
        iterations_run=iterations_run,
    )
