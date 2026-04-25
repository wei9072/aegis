"""
SemanticComparator — pluggable semantic-overlap engine.

The comparator is dependency-injected so cost-bearing layers
(IntentBypassDetector, ToolCallValidator Tier-2) stay testable without
a live LLM. Two implementations live here:

  - `SemanticComparator` (Protocol): the public contract.
  - `StubSemanticComparator`: deterministic, scenario-friendly. Returns
    a fixed result, or a per-call mapping when the test wants to vary
    overlap based on what the layer asks about.

The real LLM-backed implementation belongs next to the provider that
hosts it; this module keeps zero LLM dependencies on purpose.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Mapping, Protocol, runtime_checkable


@dataclass(frozen=True)
class SemanticResult:
    """Output of one comparison.

    `overlap` is a unitless score in [0.0, 1.0]; callers compare it
    against their own threshold (the comparator does not gate). A
    short `rationale` keeps decisions auditable in the trace.
    """

    overlap: float
    rationale: str = ""


@runtime_checkable
class SemanticComparator(Protocol):
    """How much does `b` semantically satisfy `a`?

    `context` is an optional hint (e.g. the name of the layer asking)
    so a real LLM-backed comparator can prompt itself appropriately.
    Stub implementations are free to ignore it.
    """

    def compare(self, a: str, b: str, *, context: str = "") -> SemanticResult: ...


class StubSemanticComparator:
    """Deterministic comparator for tests and eval scenarios.

    Two modes:
      - fixed: always returns `(overlap, rationale)`.
      - mapping: returns the result whose key (a substring of either
        `a` or `b`) is found in the inputs; falls back to `default`.
    """

    def __init__(
        self,
        overlap: float = 0.0,
        rationale: str = "stub",
        *,
        mapping: Mapping[str, SemanticResult] | None = None,
        default: SemanticResult | None = None,
    ) -> None:
        self._fixed = SemanticResult(overlap=overlap, rationale=rationale)
        self._mapping = dict(mapping or {})
        self._default = default

    def compare(self, a: str, b: str, *, context: str = "") -> SemanticResult:
        for key, result in self._mapping.items():
            if key in a or key in b:
                return result
        if self._mapping and self._default is not None:
            return self._default
        return self._fixed
