"""
Executor: atomically apply a PatchPlan, with backup + rollback on failure.

Re-export module — the implementation lives in `aegis._core` (Rust,
via the aegis-pyshim crate) since the V1.2 follow-up port. Public
surface (`Executor`, `ExecutionResult`, `PatchResult`) is identical
to V0.x; downstream code that did
`from aegis.runtime.executor import Executor, ExecutionResult,
PatchResult` continues to work unchanged.

Behaviour parity preserved:
  - in-memory snapshot dict is built from `plan.patches[*].path`,
    deduped in plan order
  - backup tree is written under `<root>/<backup_subdir>/<timestamp>/`
  - on failure, every touched file is restored AND any newly-created
    file is removed
  - `rollback_result(result)` restores from `result.backup_dir`
  - older backup directories beyond `keep_backups` are GC'd
"""
from __future__ import annotations

from aegis._core import ExecutionResult, Executor, PatchResult

__all__ = ["ExecutionResult", "Executor", "PatchResult"]
