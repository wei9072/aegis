"""
PlanValidator: gate between Planner and Executor.

Re-export module — the implementation lives in `aegis._core` (Rust,
via the aegis-pyshim crate) since the V1.2 follow-up port. Public
surface (`PlanValidator`, `ValidationError`, `ErrorKind`) is identical
to V0.x.

The seven `ErrorKind` discriminants kept their lowercase string values
(`"schema"`, `"path"`, `"scope"`, `"dangerous_path"`,
`"simulate_not_found"`, `"simulate_ambiguous"`, `"simulate_conflict"`)
so existing callers pattern-matching on `e.kind == "path"` etc. work
unchanged.
"""
from __future__ import annotations

from typing import Literal

from aegis._core import PlanValidator, ValidationError

ErrorKind = Literal[
    "schema",
    "path",
    "scope",
    "dangerous_path",
    "simulate_not_found",
    "simulate_ambiguous",
    "simulate_conflict",
]

__all__ = ["ErrorKind", "PlanValidator", "ValidationError"]
