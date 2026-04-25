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
