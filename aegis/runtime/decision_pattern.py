"""
DecisionPattern — the named shapes of one pipeline iteration.

Re-export module. Implementation lives in `aegis._core` (Rust, via
aegis-pyshim) since V1.0 of the Rust port. Public API unchanged:

  - `DecisionPattern` is a PyO3 enum with the same string values
    as the V0.x `str, Enum`. `DecisionPattern.APPLIED_DONE.value ==
    "applied_done"`. Callers can compare against strings directly
    (`pattern == "applied_done"`).
  - `derive_pattern(ev)` reads the same attribute set on `ev` as the
    V0.x function.

PyO3 0.20 enums don't expose metaclass `__iter__`, so callers that
want enumeration use `DecisionPattern.members()` instead of
`for p in DecisionPattern`. The Rust `derive_pattern` is exhaustive
over the same branches the Python version was; UNKNOWN is still
the safety valve that signals a missing branch.
"""
from __future__ import annotations

from aegis._core import DecisionPattern, derive_pattern

__all__ = ["DecisionPattern", "derive_pattern"]
