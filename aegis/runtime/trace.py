"""
DecisionTrace: structured record of every decision made during a single request.

Re-export module — the implementation lives in `aegis._core` (Rust,
via aegis-pyshim crate) since the V1.0 phase of the Rust port. The
public API shape is identical to V0.x; downstream code that did
`from aegis.runtime.trace import DecisionTrace, DecisionEvent, PASS,
BLOCK, WARN, OBSERVE` continues to work unchanged.

The four verb constants and the design rules they encode (events
append-only, totally ordered by emission; layer names open strings;
verbs constrained to PASS/BLOCK/WARN/OBSERVE; pure data, no I/O)
still apply — they're enforced at the Rust layer now.
"""
from __future__ import annotations

from aegis._core import (
    BLOCK,
    OBSERVE,
    PASS,
    WARN,
    DecisionEvent,
    DecisionTrace,
)

__all__ = [
    "BLOCK",
    "OBSERVE",
    "PASS",
    "WARN",
    "DecisionEvent",
    "DecisionTrace",
]
