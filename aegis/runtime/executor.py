"""
Executor: atomically apply a PatchPlan, with backup + rollback on failure.

Assumes plan has already passed PlanValidator. Still re-verifies at write
time (disk may have changed since validation) and rolls back on any issue.
"""
from __future__ import annotations

import shutil
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

from aegis.ir.patch import Patch, PatchKind, PatchPlan, PatchStatus
from aegis.shared.edit_engine import apply_edits, is_ok


@dataclass
class PatchResult:
    patch_id: str
    status: PatchStatus
    matches: int = 0
    error: str | None = None


@dataclass
class ExecutionResult:
    success: bool
    results: list[PatchResult] = field(default_factory=list)
    backup_dir: str | None = None
    rolled_back: bool = False
    staleness_detected: bool = False  # reserved for future TOCTOU hash check
    created_paths: list[str] = field(default_factory=list)
    touched_paths: list[str] = field(default_factory=list)
    # Final on-disk content for each touched path. Populated by Executor
    # so ToolCallValidator Tier-2 can compare LLM narration against what
    # actually got written without reading live state itself (invariant 6:
    # the decision phase consumes only the executor-provided snapshot).
    path_contents: dict[str, str] = field(default_factory=dict)


class Executor:
    def __init__(
        self,
        root: str,
        backup_subdir: str = ".aegis/backups",
        keep_backups: int = 5,
    ) -> None:
        self.root = Path(root).resolve()
        self.backup_subdir = backup_subdir
        self.keep_backups = keep_backups

    def apply(self, plan: PatchPlan) -> ExecutionResult:
        backup_dir = self._make_backup_dir()
        snapshot: dict[str, str | None] = {}

        try:
            self._take_snapshot(plan, snapshot, backup_dir)
            results, failed, current = self._apply_patches(plan, snapshot)
        except Exception as e:
            self._rollback(snapshot)
            return ExecutionResult(
                success=False,
                results=[PatchResult(
                    patch_id="<pre-apply>", status=PatchStatus.NOT_FOUND, error=str(e)
                )],
                backup_dir=str(backup_dir),
                rolled_back=True,
            )

        if failed:
            self._rollback(snapshot)
            return ExecutionResult(
                success=False,
                results=results,
                backup_dir=str(backup_dir),
                rolled_back=True,
            )

        created = [p for p, original in snapshot.items() if original is None]
        touched = list(snapshot.keys())
        # Final content per path — None entries are deletes, skip those.
        path_contents = {p: c for p, c in current.items() if c is not None}
        self._gc_backups()
        return ExecutionResult(
            success=True,
            results=results,
            backup_dir=str(backup_dir),
            created_paths=created,
            touched_paths=touched,
            path_contents=path_contents,
        )

    def _make_backup_dir(self) -> Path:
        backup_root = self.root / self.backup_subdir
        backup_root.mkdir(parents=True, exist_ok=True)
        prefix = time.strftime("%Y%m%d-%H%M%S-")
        return Path(tempfile.mkdtemp(prefix=prefix, dir=backup_root))

    def _take_snapshot(
        self, plan: PatchPlan, snapshot: dict[str, str | None], backup_dir: Path
    ) -> None:
        for patch in plan.patches:
            if patch.path in snapshot:
                continue
            abs_path = self.root / patch.path
            if abs_path.is_file():
                original = abs_path.read_text(encoding="utf-8")
                snapshot[patch.path] = original
                backup_file = backup_dir / patch.path
                backup_file.parent.mkdir(parents=True, exist_ok=True)
                backup_file.write_text(original, encoding="utf-8")
            else:
                snapshot[patch.path] = None

    def _apply_patches(
        self, plan: PatchPlan, snapshot: dict[str, str | None]
    ) -> tuple[list[PatchResult], bool, dict[str, str | None]]:
        """Returns (results, failed, current). `current` carries the
        final post-apply content per path so the caller can build
        `ExecutionResult.path_contents`. Stops on first failure."""
        current: dict[str, str | None] = dict(snapshot)
        results: list[PatchResult] = []

        for patch in plan.patches:
            try:
                result = self._apply_one(patch, current)
            except Exception as e:
                results.append(PatchResult(
                    patch_id=patch.id, status=PatchStatus.NOT_FOUND, error=str(e)
                ))
                return results, True, current
            results.append(result)
            if not is_ok(result.status):
                return results, True, current
        return results, False, current

    def _apply_one(
        self, patch: Patch, current: dict[str, str | None]
    ) -> PatchResult:
        abs_path = self.root / patch.path
        state = current.get(patch.path)

        if patch.kind == PatchKind.CREATE:
            if state is not None or abs_path.exists():
                return PatchResult(
                    patch_id=patch.id,
                    status=PatchStatus.NOT_FOUND,
                    error=f"CREATE target already exists: {patch.path}",
                )
            content = patch.content or ""
            abs_path.parent.mkdir(parents=True, exist_ok=True)
            abs_path.write_text(content, encoding="utf-8")
            current[patch.path] = content
            return PatchResult(patch_id=patch.id, status=PatchStatus.APPLIED, matches=1)

        if patch.kind == PatchKind.MODIFY:
            if state is None:
                return PatchResult(
                    patch_id=patch.id,
                    status=PatchStatus.NOT_FOUND,
                    error=f"MODIFY target missing: {patch.path}",
                )
            new_content, edit_results = apply_edits(state, patch.edits)
            for er in edit_results:
                if not is_ok(er.status):
                    return PatchResult(
                        patch_id=patch.id, status=er.status, matches=er.matches
                    )
            any_applied = any(
                er.status == PatchStatus.APPLIED for er in edit_results
            )
            overall = (
                PatchStatus.APPLIED if any_applied else PatchStatus.ALREADY_APPLIED
            )
            if new_content != state:
                abs_path.write_text(new_content, encoding="utf-8")
            current[patch.path] = new_content
            return PatchResult(patch_id=patch.id, status=overall, matches=1)

        if patch.kind == PatchKind.DELETE:
            if state is None:
                return PatchResult(
                    patch_id=patch.id,
                    status=PatchStatus.ALREADY_APPLIED,
                    matches=1,
                )
            abs_path.unlink()
            current[patch.path] = None
            return PatchResult(patch_id=patch.id, status=PatchStatus.APPLIED, matches=1)

        return PatchResult(
            patch_id=patch.id,
            status=PatchStatus.NOT_FOUND,
            error=f"unknown patch kind: {patch.kind}",
        )

    def _rollback(self, snapshot: dict[str, str | None]) -> None:
        for rel_path, original in snapshot.items():
            abs_path = self.root / rel_path
            if original is None:
                if abs_path.exists():
                    abs_path.unlink()
            else:
                abs_path.parent.mkdir(parents=True, exist_ok=True)
                abs_path.write_text(original, encoding="utf-8")

    def rollback_result(self, result: ExecutionResult) -> None:
        """Restore on-disk state to what it was before `result`.

        Restores every file present in backup_dir, and deletes anything
        we newly created. Idempotent.
        """
        if result.backup_dir is None:
            return
        bp = Path(result.backup_dir)
        if bp.is_dir():
            for backup_file in bp.rglob("*"):
                if not backup_file.is_file():
                    continue
                rel = backup_file.relative_to(bp)
                target = self.root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(
                    backup_file.read_text(encoding="utf-8"), encoding="utf-8"
                )
        for rel_path in result.created_paths:
            abs_path = self.root / rel_path
            if abs_path.exists():
                abs_path.unlink()

    def _gc_backups(self) -> None:
        backup_root = self.root / self.backup_subdir
        if not backup_root.is_dir():
            return
        dirs = sorted(d for d in backup_root.iterdir() if d.is_dir())
        for old in dirs[: -self.keep_backups] if len(dirs) > self.keep_backups else []:
            shutil.rmtree(old, ignore_errors=True)
