"""
PlanValidator: gate between Planner and Executor.

Catches schema errors, path-safety violations, scope escapes, and
cross-patch conflicts (via virtual-filesystem simulation) BEFORE any
byte touches disk.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from aegis.ir.patch import Patch, PatchKind, PatchPlan, PatchStatus
from aegis.shared.edit_engine import apply_edits, is_ok


ErrorKind = Literal[
    "schema",
    "path",
    "scope",
    "dangerous_path",
    "simulate_not_found",
    "simulate_ambiguous",
    "simulate_conflict",
]


@dataclass
class ValidationError:
    kind: ErrorKind
    message: str
    patch_id: str | None = None
    edit_index: int | None = None
    matches: int = 0


_FORBIDDEN_PARTS = frozenset({".git", ".aegis", ".venv", "venv", "__pycache__",
                              "node_modules", ".hg", ".svn", ".idea", ".vscode"})


class PlanValidator:
    def __init__(self, root: str, scope: list[str] | None = None) -> None:
        self.root = Path(root).resolve()
        self.scope: list[Path] | None = (
            [self._resolve_under_root(s) for s in scope] if scope else None
        )

    def validate(self, plan: PatchPlan) -> list[ValidationError]:
        errors: list[ValidationError] = []
        errors += self._check_plan_shape(plan)
        for patch in plan.patches:
            errors += self._check_patch_schema(patch)
            errors += self._check_patch_path(patch)
            errors += self._check_patch_scope(patch)
            errors += self._check_target_files_commitment(patch, plan)
        if errors:
            return errors
        errors += self._simulate(plan)
        return errors

    def _check_plan_shape(self, plan: PatchPlan) -> list[ValidationError]:
        errs: list[ValidationError] = []
        if not plan.patches:
            errs.append(ValidationError(
                kind="schema", message="plan has no patches"
            ))
        ids = [p.id for p in plan.patches]
        if len(ids) != len(set(ids)):
            errs.append(ValidationError(
                kind="schema", message="patch ids are not unique"
            ))
        return errs

    def _check_patch_schema(self, patch: Patch) -> list[ValidationError]:
        errs: list[ValidationError] = []
        if not patch.id:
            errs.append(ValidationError(
                kind="schema", message="patch missing id", patch_id=None
            ))
        if not patch.path:
            errs.append(ValidationError(
                kind="schema", message="patch missing path", patch_id=patch.id
            ))
        if patch.kind == PatchKind.CREATE:
            if patch.content is None:
                errs.append(ValidationError(
                    kind="schema",
                    message="CREATE patch missing content",
                    patch_id=patch.id,
                ))
            if patch.edits:
                errs.append(ValidationError(
                    kind="schema",
                    message="CREATE patch must not have edits",
                    patch_id=patch.id,
                ))
        elif patch.kind == PatchKind.MODIFY:
            if not patch.edits:
                errs.append(ValidationError(
                    kind="schema",
                    message="MODIFY patch must have at least one edit",
                    patch_id=patch.id,
                ))
            for i, edit in enumerate(patch.edits):
                if not edit.old_string:
                    errs.append(ValidationError(
                        kind="schema",
                        message="edit has empty old_string",
                        patch_id=patch.id,
                        edit_index=i,
                    ))
                if not edit.context_before and not edit.context_after:
                    errs.append(ValidationError(
                        kind="schema",
                        message=(
                            "edit missing context_before and context_after "
                            "(at least one required for MODIFY)"
                        ),
                        patch_id=patch.id,
                        edit_index=i,
                    ))
        elif patch.kind == PatchKind.DELETE:
            if patch.content is not None or patch.edits:
                errs.append(ValidationError(
                    kind="schema",
                    message="DELETE patch must not carry content or edits",
                    patch_id=patch.id,
                ))
        return errs

    def _check_patch_path(self, patch: Patch) -> list[ValidationError]:
        if not patch.path:
            return []
        try:
            resolved = self._resolve_under_root(patch.path)
        except ValueError as e:
            return [ValidationError(
                kind="path", message=str(e), patch_id=patch.id
            )]
        for part in resolved.parts:
            if part in _FORBIDDEN_PARTS:
                return [ValidationError(
                    kind="dangerous_path",
                    message=f"path crosses forbidden directory: {part}",
                    patch_id=patch.id,
                )]
        return []

    def _check_patch_scope(self, patch: Patch) -> list[ValidationError]:
        if self.scope is None or not patch.path:
            return []
        try:
            resolved = self._resolve_under_root(patch.path)
        except ValueError:
            return []  # already reported by _check_patch_path
        for allowed in self.scope:
            try:
                resolved.relative_to(allowed)
                return []
            except ValueError:
                continue
        return [ValidationError(
            kind="scope",
            message=f"patch path {patch.path} outside declared scope",
            patch_id=patch.id,
        )]

    def _check_target_files_commitment(
        self, patch: Patch, plan: PatchPlan
    ) -> list[ValidationError]:
        """Every patch.path must appear in plan.target_files (if declared).

        `target_files` is a commitment mechanism: the planner declares its
        intended blast radius, and we reject patches outside that declaration.
        If empty, no check (planner opted out of the commitment).
        """
        if not plan.target_files or not patch.path:
            return []
        if patch.path in plan.target_files:
            return []
        return [ValidationError(
            kind="scope",
            message=(
                f"patch path {patch.path} not in declared target_files "
                f"{plan.target_files}"
            ),
            patch_id=patch.id,
        )]

    def _simulate(self, plan: PatchPlan) -> list[ValidationError]:
        errs: list[ValidationError] = []
        virtual: dict[str, str | None] = {}

        def load(rel_path: str) -> str | None:
            if rel_path in virtual:
                return virtual[rel_path]
            abs_path = self.root / rel_path
            if not abs_path.exists():
                return None
            try:
                return abs_path.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                return None

        for patch in plan.patches:
            current = load(patch.path)
            if patch.kind == PatchKind.CREATE:
                if current is not None:
                    errs.append(ValidationError(
                        kind="simulate_conflict",
                        message=f"CREATE target already exists: {patch.path}",
                        patch_id=patch.id,
                    ))
                    continue
                virtual[patch.path] = patch.content or ""
            elif patch.kind == PatchKind.MODIFY:
                if current is None:
                    errs.append(ValidationError(
                        kind="simulate_conflict",
                        message=f"MODIFY target missing: {patch.path}",
                        patch_id=patch.id,
                    ))
                    continue
                new_content, results = apply_edits(current, patch.edits)
                for i, res in enumerate(results):
                    if is_ok(res.status):
                        continue
                    kind: ErrorKind = (
                        "simulate_ambiguous"
                        if res.status == PatchStatus.AMBIGUOUS
                        else "simulate_not_found"
                    )
                    errs.append(ValidationError(
                        kind=kind,
                        message=f"edit {i} {res.status.value} (matches={res.matches})",
                        patch_id=patch.id,
                        edit_index=i,
                        matches=res.matches,
                    ))
                virtual[patch.path] = new_content
            elif patch.kind == PatchKind.DELETE:
                if current is None:
                    errs.append(ValidationError(
                        kind="simulate_conflict",
                        message=f"DELETE target missing: {patch.path}",
                        patch_id=patch.id,
                    ))
                    continue
                virtual[patch.path] = None
        return errs

    def _resolve_under_root(self, rel_or_abs: str) -> Path:
        p = Path(rel_or_abs)
        resolved = (self.root / p).resolve() if not p.is_absolute() else p.resolve()
        try:
            resolved.relative_to(self.root)
        except ValueError:
            raise ValueError(f"path {rel_or_abs} escapes project root")
        return resolved
