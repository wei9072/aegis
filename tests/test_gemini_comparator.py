"""
Unit tests for GeminiSemanticComparator.

We only test the parser + provider plumbing — never the live model.
A FakeProvider lets us pin every parser branch (well-formed JSON,
embedded JSON inside prose, malformed JSON, missing field,
out-of-range value, empty response).
"""
from __future__ import annotations

from aegis.semantic.gemini_comparator import GeminiSemanticComparator


class _FakeProvider:
    last_used_tools: tuple = ()

    def __init__(self, response: str) -> None:
        self._response = response
        self.calls: list[tuple[str, tuple]] = []

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        self.calls.append((prompt, tools or ()))
        self.last_used_tools = tuple(tools) if tools is not None else ()
        return self._response


def test_clean_json_is_parsed():
    fake = _FakeProvider('{"overlap": 0.92, "rationale": "bypass demo"}')
    r = GeminiSemanticComparator(provider=fake).compare("a", "b", context="x")
    assert r.overlap == 0.92
    assert r.rationale == "bypass demo"


def test_json_embedded_in_prose_is_extracted():
    fake = _FakeProvider(
        'Here is my judgement:\n{"overlap": 0.4, "rationale": "partial"}\nDone.'
    )
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 0.4
    assert r.rationale == "partial"


def test_overlap_above_one_is_clamped():
    fake = _FakeProvider('{"overlap": 1.5, "rationale": "drift"}')
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 1.0


def test_overlap_below_zero_is_clamped():
    fake = _FakeProvider('{"overlap": -0.2, "rationale": "drift"}')
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 0.0


def test_unparseable_response_falls_open_to_zero():
    fake = _FakeProvider("the response is interesting but no json here")
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 0.0
    assert "unparseable" in r.rationale


def test_empty_response_falls_open_to_zero():
    fake = _FakeProvider("")
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 0.0


def test_malformed_json_falls_open_to_zero():
    fake = _FakeProvider('{"overlap": 0.5, "rationale": "missing brace"')
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 0.0
    assert "unparseable" in r.rationale or "json" in r.rationale.lower()


def test_overlap_field_not_numeric_falls_open():
    fake = _FakeProvider('{"overlap": "high", "rationale": "wat"}')
    r = GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert r.overlap == 0.0


def test_comparator_passes_empty_tool_surface_to_provider():
    """Comparator must never expose tools — pure text reasoning only."""
    fake = _FakeProvider('{"overlap": 0.0, "rationale": "ok"}')
    GeminiSemanticComparator(provider=fake).compare("a", "b")
    assert fake.calls[0][1] == ()


def test_context_string_appears_in_prompt():
    """Caller-provided framing must reach the LLM verbatim."""
    fake = _FakeProvider('{"overlap": 0.0, "rationale": ""}')
    GeminiSemanticComparator(provider=fake).compare("a", "b", context="MY_CTX")
    assert "MY_CTX" in fake.calls[0][0]


def test_lazy_provider_does_not_instantiate_at_construction():
    """No API key needed to import / instantiate the comparator."""
    # Should not raise even if GEMINI_API_KEY is absent at this line.
    c = GeminiSemanticComparator()
    assert c._provider is None  # noqa: SLF001 — testing the lazy contract
