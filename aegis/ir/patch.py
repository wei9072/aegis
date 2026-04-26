"""
PatchPlan: structured intent shared between Planner and Executor.

Re-export module — the implementation lives in `aegis._core` (Rust,
via the aegis-pyshim crate) since the V1.3 IR-model port. The
public API shape is identical to V0.x; downstream code that did
`from aegis.ir.patch import Edit, Patch, PatchPlan, PatchKind,
PatchStatus, plan_to_dict, plan_from_dict` continues to work
unchanged.

The dataclass-style construction (`Edit(old_string=..., new_string=...)`)
and field access (`patch.kind`, `plan.patches`) still work — the
PyO3 classes mirror the same surface. The two str-Enums also keep
their string-equality behaviour (`patch.kind == "modify"`).
"""
from __future__ import annotations

from aegis._core import (
    Edit,
    EditResult,
    Patch,
    PatchKind,
    PatchPlan,
    PatchStatus,
    patch_from_dict,
    patch_to_dict,
    plan_from_dict,
    plan_to_dict,
)

__all__ = [
    "Edit",
    "EditResult",
    "Patch",
    "PatchKind",
    "PatchPlan",
    "PatchStatus",
    "patch_from_dict",
    "patch_to_dict",
    "plan_from_dict",
    "plan_to_dict",
]
