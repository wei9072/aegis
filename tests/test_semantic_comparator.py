"""
Unit tests for SemanticComparator and StubSemanticComparator.

The stub is the lifeline that keeps Phase-3 layers testable without an
LLM. Its behaviour must stay predictable: fixed mode is fixed; mapping
mode picks the first key that appears in either input; default falls
back to the constructor's `default` (or, if no mapping, the fixed
result).
"""
from __future__ import annotations

from aegis.semantic.comparator import (
    SemanticComparator,
    SemanticResult,
    StubSemanticComparator,
)


def test_stub_in_fixed_mode_returns_constant():
    stub = StubSemanticComparator(overlap=0.42, rationale="r")
    r = stub.compare("anything", "else")
    assert r.overlap == 0.42
    assert r.rationale == "r"


def test_stub_mapping_picks_matching_key():
    stub = StubSemanticComparator(
        mapping={
            "fibonacci": SemanticResult(0.9, "fib"),
            "list comprehension": SemanticResult(0.1, "benign"),
        },
        default=SemanticResult(0.0, "no match"),
    )
    assert stub.compare("show me fibonacci", "code").overlap == 0.9
    assert stub.compare("a", "list comprehension example").overlap == 0.1


def test_stub_mapping_falls_back_to_default():
    stub = StubSemanticComparator(
        mapping={"fibonacci": SemanticResult(0.9, "fib")},
        default=SemanticResult(0.0, "default"),
    )
    r = stub.compare("unrelated", "request")
    assert r.overlap == 0.0
    assert r.rationale == "default"


def test_stub_satisfies_protocol():
    """Runtime protocol check — a misnamed method would break callers."""
    stub = StubSemanticComparator()
    assert isinstance(stub, SemanticComparator)


def test_semantic_result_is_immutable():
    r = SemanticResult(overlap=0.5, rationale="x")
    try:
        r.overlap = 0.9  # type: ignore[misc]
    except Exception:
        return
    raise AssertionError("SemanticResult must be frozen")
