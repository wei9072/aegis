"""
Pure edit-application logic shared by Validator (virtual-fs simulation) and
Executor (real write). No I/O. Deterministic. Single source of truth for
APPLIED / ALREADY_APPLIED / AMBIGUOUS / NOT_FOUND semantics.
"""
from __future__ import annotations

from dataclasses import dataclass

from aegis.ir.patch import Edit, PatchStatus


@dataclass
class EditResult:
    status: PatchStatus
    matches: int


def apply_edit(content: str, edit: Edit) -> tuple[str, EditResult]:
    """Apply a single edit to an in-memory string.

    Anchored semantics (preferred — used whenever context_before or
    context_after is non-empty):
      - `context_before + old_string + context_after` uniquely present
        -> APPLIED, return modified content.
      - `context_before + new_string + context_after` uniquely present
        (and old anchor absent) -> ALREADY_APPLIED.
      - Either anchor appears multiple times -> AMBIGUOUS.
      - Neither anchor present -> NOT_FOUND.

    Using the anchor for BOTH paths avoids prefix-overlap errors
    (e.g. replacing "x = 1" inside "x = 10").

    Unanchored fallback (raw old_string only) exists solely for safety
    if an unvalidated plan slips through; the Validator rejects MODIFY
    edits without context, so production paths always hit the anchored
    branch.
    """
    if not edit.old_string:
        return content, EditResult(status=PatchStatus.NOT_FOUND, matches=0)

    has_context = bool(edit.context_before or edit.context_after)
    if has_context:
        old_anchor = edit.context_before + edit.old_string + edit.context_after
        new_anchor = edit.context_before + edit.new_string + edit.context_after

        old_count = content.count(old_anchor)
        if old_count == 1:
            new_content = content.replace(old_anchor, new_anchor, 1)
            return new_content, EditResult(status=PatchStatus.APPLIED, matches=1)
        if old_count > 1:
            return content, EditResult(status=PatchStatus.AMBIGUOUS, matches=old_count)

        new_count = content.count(new_anchor)
        if new_count == 1:
            return content, EditResult(status=PatchStatus.ALREADY_APPLIED, matches=1)
        if new_count > 1:
            return content, EditResult(status=PatchStatus.AMBIGUOUS, matches=new_count)

        return content, EditResult(status=PatchStatus.NOT_FOUND, matches=0)

    # Unanchored fallback: old_string alone must be unique.
    raw_count = content.count(edit.old_string)
    if raw_count == 1:
        new_content = content.replace(edit.old_string, edit.new_string, 1)
        return new_content, EditResult(status=PatchStatus.APPLIED, matches=1)
    if raw_count > 1:
        return content, EditResult(status=PatchStatus.AMBIGUOUS, matches=raw_count)
    return content, EditResult(status=PatchStatus.NOT_FOUND, matches=0)


def apply_edits(content: str, edits: list[Edit]) -> tuple[str, list[EditResult]]:
    """Sequentially apply edits. Each edit sees the state left by prior edits.

    Failed edits leave content unchanged for that step; subsequent edits
    still evaluate against the current state. Caller decides what to do
    with any non-{APPLIED, ALREADY_APPLIED} result.
    """
    results: list[EditResult] = []
    for edit in edits:
        content, result = apply_edit(content, edit)
        results.append(result)
    return content, results


def is_ok(status: PatchStatus) -> bool:
    """Valid outcomes that do NOT require rollback."""
    return status in (PatchStatus.APPLIED, PatchStatus.ALREADY_APPLIED)
