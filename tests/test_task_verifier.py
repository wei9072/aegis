"""
Unit tests for aegis.runtime.task_verifier.

These tests pin the Layer C design rules established in
docs/v1_validation.md#framing and the
aegis_core_framing_negative_space.md memory entry. The most important
ones to keep green:

  - apply_verifier never raises, even when the verifier raises
  - derive_task_pattern is exhaustive over inputs
  - verifier output goes nowhere except TaskVerdict (negatively
    pinned: TaskVerdict has no field named anything like
    "feeds_back_to_planner")

A future refactor that breaks these is a refactor that breaks the
"negative-space rejection harness" framing — these tests are the
fence.
"""
from __future__ import annotations

from pathlib import Path

import pytest

from aegis.runtime.task_verifier import (
    TaskPattern,
    TaskVerdict,
    TaskVerifier,
    VerifierResult,
    apply_verifier,
    derive_task_pattern,
)


class _PassingVerifier:
    def verify(self, workspace, trace) -> VerifierResult:
        return VerifierResult(passed=True, rationale="ok", evidence={"k": 1})


class _FailingVerifier:
    def verify(self, workspace, trace) -> VerifierResult:
        return VerifierResult(passed=False, rationale="nope", evidence={"why": "bad"})


class _RaisingVerifier:
    def verify(self, workspace, trace) -> VerifierResult:
        raise RuntimeError("verifier blew up")


# ---------- derive_task_pattern (pure mapping) ----------

def test_derive_no_verifier_present():
    assert derive_task_pattern(
        verifier_present=False, verifier_passed=None, verifier_raised=False,
        pipeline_done=True,
    ) is TaskPattern.NO_VERIFIER


def test_derive_verifier_raised():
    assert derive_task_pattern(
        verifier_present=True, verifier_passed=None, verifier_raised=True,
        pipeline_done=True,
    ) is TaskPattern.VERIFIER_ERROR


def test_derive_solved_regardless_of_pipeline_done():
    """SOLVED only depends on the verifier — actual outcome wins.
    If the LLM gave up but the workspace is in fact solved, we report
    SOLVED. Pipeline-done state is just diagnostic context."""
    for pipeline_done in (True, False):
        assert derive_task_pattern(
            verifier_present=True, verifier_passed=True, verifier_raised=False,
            pipeline_done=pipeline_done,
        ) is TaskPattern.SOLVED


def test_derive_incomplete_vs_abandoned():
    """The split between INCOMPLETE and ABANDONED is purely whether
    the planner declared done. Both verifier-failed; the difference
    is honesty."""
    assert derive_task_pattern(
        verifier_present=True, verifier_passed=False, verifier_raised=False,
        pipeline_done=True,
    ) is TaskPattern.INCOMPLETE
    assert derive_task_pattern(
        verifier_present=True, verifier_passed=False, verifier_raised=False,
        pipeline_done=False,
    ) is TaskPattern.ABANDONED


# ---------- apply_verifier (always returns a TaskVerdict) ----------

def test_apply_verifier_none_yields_no_verifier(tmp_path: Path):
    verdict = apply_verifier(
        verifier=None, workspace=tmp_path, trace=[],
        pipeline_done=True, iterations_run=2,
    )
    assert verdict.pattern is TaskPattern.NO_VERIFIER
    assert verdict.verifier_result is None
    assert verdict.pipeline_done is True
    assert verdict.iterations_run == 2


def test_apply_verifier_passing(tmp_path: Path):
    verdict = apply_verifier(
        verifier=_PassingVerifier(), workspace=tmp_path, trace=[],
        pipeline_done=True, iterations_run=1,
    )
    assert verdict.pattern is TaskPattern.SOLVED
    assert verdict.verifier_result is not None
    assert verdict.verifier_result.passed is True
    assert verdict.verifier_result.evidence == {"k": 1}


def test_apply_verifier_failing_pipeline_done(tmp_path: Path):
    verdict = apply_verifier(
        verifier=_FailingVerifier(), workspace=tmp_path, trace=[],
        pipeline_done=True, iterations_run=3,
    )
    assert verdict.pattern is TaskPattern.INCOMPLETE
    assert verdict.verifier_result.rationale == "nope"


def test_apply_verifier_failing_pipeline_not_done(tmp_path: Path):
    verdict = apply_verifier(
        verifier=_FailingVerifier(), workspace=tmp_path, trace=[],
        pipeline_done=False, iterations_run=3,
    )
    assert verdict.pattern is TaskPattern.ABANDONED


def test_apply_verifier_swallows_exception(tmp_path: Path):
    """A buggy verifier must not crash the post-pipeline path. The
    error is preserved so the snapshot retains the diagnostic."""
    verdict = apply_verifier(
        verifier=_RaisingVerifier(), workspace=tmp_path, trace=[],
        pipeline_done=True, iterations_run=1,
    )
    assert verdict.pattern is TaskPattern.VERIFIER_ERROR
    assert verdict.verifier_result is None
    assert "RuntimeError" in verdict.error
    assert "verifier blew up" in verdict.error


# ---------- TaskVerdict shape (the Layer B/C isolation contract) ----------

def test_task_verdict_has_no_feedback_field():
    """**Critical contract**: TaskVerdict must not carry any field
    that suggests the verdict could be fed back into the loop. If
    someone adds a field like `should_retry` or `next_plan_hint` or
    `feedback_for_planner`, this test should fail and force a
    framing-level conversation before the merge.

    See `docs/v1_validation.md#framing` design rule #3.
    """
    fields = {f.name for f in TaskVerdict.__dataclass_fields__.values()}
    forbidden_substrings = (
        "retry", "feedback", "hint", "next_plan", "advice", "guidance",
    )
    for field_name in fields:
        for forbidden in forbidden_substrings:
            assert forbidden not in field_name.lower(), (
                f"TaskVerdict field {field_name!r} contains forbidden "
                f"substring {forbidden!r} — this looks like a feedback "
                f"channel. See Layer B/C isolation rule in "
                f"docs/v1_validation.md#framing"
            )


def test_task_verifier_protocol_only_has_verify_method():
    """TaskVerifier Protocol must remain a single-method interface.
    Adding methods that the loop could call mid-iteration would
    blur Layer B/C isolation."""
    callable_attrs = [
        a for a in dir(TaskVerifier)
        if not a.startswith("_") and callable(getattr(TaskVerifier, a, None))
    ]
    assert callable_attrs == ["verify"], (
        f"TaskVerifier should only expose .verify(); got {callable_attrs}. "
        f"New methods on this Protocol risk being called mid-loop, which "
        f"would let verifier output reach Layer B."
    )


def test_to_dict_preserves_all_fields(tmp_path: Path):
    verdict = apply_verifier(
        verifier=_PassingVerifier(), workspace=tmp_path, trace=[],
        pipeline_done=True, iterations_run=2,
    )
    d = verdict.to_dict()
    assert d["pattern"] == "solved"
    assert d["pipeline_done"] is True
    assert d["iterations_run"] == 2
    assert d["error"] == ""
    assert d["verifier_result"]["passed"] is True
    assert d["verifier_result"]["rationale"] == "ok"
    assert d["verifier_result"]["evidence"] == {"k": 1}


def test_to_dict_no_verifier_case(tmp_path: Path):
    verdict = apply_verifier(
        verifier=None, workspace=tmp_path, trace=[],
        pipeline_done=False, iterations_run=0,
    )
    d = verdict.to_dict()
    assert d["pattern"] == "no_verifier"
    assert d["verifier_result"] is None
