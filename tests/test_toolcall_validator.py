"""
Unit tests for ToolCallValidator (Tier-1).

Tier-1 is purely deterministic — pattern match a write claim and compare
against ExecutionResult. These tests pin the boundary cases so future
work on Tier-2 (semantic comparison) doesn't accidentally relax the
deterministic floor.
"""
from __future__ import annotations

from aegis.runtime.executor import ExecutionResult
from aegis.runtime.trace import BLOCK, DecisionTrace
from aegis.semantic.comparator import StubSemanticComparator
from aegis.toolcall.validator import (
    ToolCallValidator,
    claim_implies_write,
)


# ---------- claim_implies_write ----------

def test_empty_text_is_not_a_claim():
    claimed, paths = claim_implies_write("")
    assert claimed is False
    assert paths == []


def test_verb_without_path_is_not_a_claim():
    """'I created a function' without a filename must not trigger."""
    claimed, paths = claim_implies_write("I created a small helper function")
    assert claimed is False


def test_path_without_verb_is_not_a_claim():
    """'See fibonacci.py' alone does not narrate a write."""
    claimed, paths = claim_implies_write("See fibonacci.py for details.")
    assert claimed is False


def test_chinese_create_with_filename_is_a_claim():
    claimed, paths = claim_implies_write("我已經為你創建了 fibonacci.py 檔案。")
    assert claimed is True
    assert paths == ["fibonacci.py"]


def test_english_created_with_filename_is_a_claim():
    claimed, paths = claim_implies_write("I created src/utils/helpers.py for you.")
    assert claimed is True
    assert "helpers.py" in paths[0]  # may be matched as src/utils/helpers.py


def test_dedupes_repeated_paths():
    claimed, paths = claim_implies_write(
        "已經建立 a.py。然後寫入 a.py 並儲存 a.py 完成。"
    )
    assert claimed is True
    assert paths == ["a.py"]


def test_section_numbers_do_not_count_as_paths():
    """`section 3.2` should not match the path regex."""
    claimed, paths = claim_implies_write("created section 3.2 of the document")
    assert claimed is False  # no real path token


# ---------- ToolCallValidator.validate ----------

def test_no_claim_yields_no_event():
    trace = DecisionTrace()
    verdict = ToolCallValidator().validate(
        "x = 1", ExecutionResult(success=True), trace=trace,
    )
    assert verdict.events == []
    assert trace.by_layer("toolcall") == []


def test_claim_without_executor_blocks_as_hallucination():
    """Scenario-10 shape: pure-text claim, executor never invoked."""
    trace = DecisionTrace()
    verdict = ToolCallValidator().validate(
        "我已經為你創建了 fibonacci 資料夾，並寫入 fibonacci.py。",
        executor_result=None,
        trace=trace,
    )
    assert verdict.has_block()
    ev = verdict.events[0]
    assert ev.decision == BLOCK
    assert ev.reason == "hallucinated_claim_no_write"
    assert ev.metadata["claimed_paths"] == ["fibonacci.py"]
    assert ev.metadata["touched_paths"] == []


def test_claim_with_empty_execution_result_also_blocks():
    """An ExecutionResult with no touched_paths is the same as no executor."""
    trace = DecisionTrace()
    verdict = ToolCallValidator().validate(
        "I wrote helper.py for you.",
        ExecutionResult(success=True, touched_paths=[]),
        trace=trace,
    )
    assert verdict.has_block()
    assert verdict.events[0].reason == "hallucinated_claim_no_write"


def test_claim_with_matching_executor_path_passes():
    trace = DecisionTrace()
    result = ExecutionResult(
        success=True,
        touched_paths=["src/fibonacci.py"],
        created_paths=["src/fibonacci.py"],
    )
    verdict = ToolCallValidator().validate(
        "I created fibonacci.py for you.", result, trace=trace,
    )
    # Suffix match: claimed "fibonacci.py" matches "src/fibonacci.py".
    assert verdict.events == []
    assert trace.by_layer("toolcall") == []


def test_claim_with_unmatched_path_blocks_with_distinct_reason():
    """Executor wrote something, but not what the LLM described."""
    trace = DecisionTrace()
    result = ExecutionResult(
        success=True,
        touched_paths=["src/foo.py"],
    )
    verdict = ToolCallValidator().validate(
        "I created bar.py.", result, trace=trace,
    )
    assert verdict.has_block()
    ev = verdict.events[0]
    assert ev.reason == "claimed_paths_not_written"
    assert ev.metadata["unmatched_paths"] == ["bar.py"]
    assert ev.metadata["touched_paths"] == ["src/foo.py"]


def test_validator_appends_to_caller_trace():
    trace = DecisionTrace()
    ToolCallValidator().validate(
        "wrote x.py", executor_result=None, trace=trace,
    )
    toolcall_events = trace.by_layer("toolcall")
    assert len(toolcall_events) == 1
    assert toolcall_events[0].decision == BLOCK


def test_validate_without_trace_still_returns_verdict():
    """`trace=None` is allowed for pure-evaluation use cases."""
    verdict = ToolCallValidator().validate(
        "I created fibonacci.py.", executor_result=None, trace=None,
    )
    # No trace means nothing to emit into; verdict.events stays empty
    # because `_emit_block` returns None when trace is None.
    assert verdict.events == []


# ---------- Tier-2 (semantic claim/content comparison) ----------

_QUICKSORT_NARRATION = (
    "我為你寫了一個 quicksort 在 sort.py。\n"
    "```python\ndef quicksort(arr): ...\n```"
)
_FIBONACCI_CONTENT = (
    "def fibonacci(n):\n    return n if n <= 1 else fibonacci(n-1) + fibonacci(n-2)"
)
_QUICKSORT_CONTENT = (
    "def quicksort(arr):\n"
    "    if len(arr) <= 1: return arr\n"
    "    p = arr[0]\n"
    "    return quicksort([x for x in arr[1:] if x < p]) + [p] + "
    "quicksort([x for x in arr[1:] if x >= p])"
)


def _matched_result(path: str, content: str) -> ExecutionResult:
    return ExecutionResult(
        success=True,
        touched_paths=[path],
        created_paths=[path],
        path_contents={path: content},
    )


def test_tier2_skipped_when_no_comparator_wired():
    """Tier-1 cleared, no comparator → no toolcall events at all."""
    trace = DecisionTrace()
    verdict = ToolCallValidator(comparator=None).validate(
        _QUICKSORT_NARRATION,
        _matched_result("sort.py", _QUICKSORT_CONTENT),
        trace=trace,
    )
    assert verdict.events == []
    assert trace.by_layer("toolcall") == []


def test_tier2_blocks_on_low_overlap():
    """Path matches, content does NOT match narration → block."""
    trace = DecisionTrace()
    cmp = StubSemanticComparator(overlap=0.12, rationale="quicksort vs fibonacci")
    verdict = ToolCallValidator(comparator=cmp, threshold=0.7).validate(
        _QUICKSORT_NARRATION,
        _matched_result("sort.py", _FIBONACCI_CONTENT),
        trace=trace,
    )
    assert verdict.has_block()
    ev = verdict.events[0]
    assert ev.decision == BLOCK
    assert ev.reason == "claim_content_mismatch"
    assert ev.metadata["matched_paths"] == ["sort.py"]
    assert ev.metadata["overlap"] == 0.12
    assert ev.metadata["threshold"] == 0.7


def test_tier2_passes_with_emit_on_high_overlap():
    """Invariant 4: pass case still emits a PASS event so the trace
    shows Tier-2 ran (and absorbed its LLM cost)."""
    trace = DecisionTrace()
    cmp = StubSemanticComparator(overlap=0.93, rationale="match")
    verdict = ToolCallValidator(comparator=cmp).validate(
        _QUICKSORT_NARRATION,
        _matched_result("sort.py", _QUICKSORT_CONTENT),
        trace=trace,
    )
    assert not verdict.has_block()
    events = trace.by_layer("toolcall")
    assert len(events) == 1
    assert events[0].decision == "pass"
    assert events[0].reason == "claim_content_matches"


def test_tier2_does_not_run_when_tier1_blocks():
    """A Tier-1 block must short-circuit before any comparator call."""

    class _ShouldNotBeCalled:
        def compare(self, *a, **kw):
            raise AssertionError("Tier-2 must not run after a Tier-1 block")

    trace = DecisionTrace()
    verdict = ToolCallValidator(comparator=_ShouldNotBeCalled()).validate(
        "I created bar.py for you.",
        # Tier-1 shape #2: claim mentions bar.py; executor wrote foo.py.
        ExecutionResult(success=True, touched_paths=["foo.py"]),
        trace=trace,
    )
    assert verdict.has_block()
    assert verdict.events[0].reason == "claimed_paths_not_written"


def test_tier2_skipped_when_no_path_content():
    """Path matched but content map empty → nothing to compare against."""
    trace = DecisionTrace()
    cmp = StubSemanticComparator(overlap=0.0)
    verdict = ToolCallValidator(comparator=cmp).validate(
        _QUICKSORT_NARRATION,
        ExecutionResult(success=True, touched_paths=["sort.py"]),  # no contents
        trace=trace,
    )
    assert verdict.events == []
    assert trace.by_layer("toolcall") == []


def test_tier2_concatenates_multiple_matched_paths():
    """When the LLM claims multiple paths, Tier-2 makes ONE comparator
    call against the concatenation — keeps cost predictable."""
    captured: list[tuple[str, str]] = []

    class _Recorder:
        def compare(self, a, b, *, context=""):
            captured.append((a, b))
            return StubSemanticComparator(overlap=0.99).compare(a, b)

    trace = DecisionTrace()
    result = ExecutionResult(
        success=True,
        touched_paths=["a.py", "b.py"],
        created_paths=["a.py", "b.py"],
        path_contents={"a.py": "AAA", "b.py": "BBB"},
    )
    ToolCallValidator(comparator=_Recorder()).validate(
        "I wrote a.py and b.py for you.",
        result,
        trace=trace,
    )
    assert len(captured) == 1, "exactly one comparator call expected"
    _, combined = captured[0]
    assert "AAA" in combined and "BBB" in combined
