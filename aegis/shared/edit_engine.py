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

    Anchor-joining is line-aware. The Planner schema describes
    context_before / context_after as "surrounding lines", and LLMs
    typically emit them as bare lines (no leading/trailing newline) —
    but the file has newlines between those lines. A pure
    byte-concatenation matcher would then fail on a perfectly correct
    plan (this was the root cause of the syntax_fix scenario refusing
    to converge: gemma-4-31b-it produced
    `context_after="    return a + b"` for `def add(a, b)`, but the
    file holds `def add(a, b)\\n    return a + b`).

    Resolution: try two candidate joinings — raw concat (for inline
    anchors like `x = 1` -> `x = 10`) and newline-joined (for the
    line-level anchors LLMs naturally produce). The first candidate
    that yields a unique match wins. If raw matches uniquely we never
    consult the newline form, so all pre-existing tests behave
    exactly as before.

    Using the anchor for BOTH old/new paths avoids prefix-overlap
    errors (e.g. replacing "x = 1" inside "x = 10").

    Unanchored fallback (raw old_string only) exists solely for safety
    if an unvalidated plan slips through; the Validator rejects MODIFY
    edits without context, so production paths always hit the anchored
    branch.
    """
    if not edit.old_string:
        return content, EditResult(status=PatchStatus.NOT_FOUND, matches=0)

    has_context = bool(edit.context_before or edit.context_after)
    if has_context:
        # Build (old_anchor, new_anchor) pairs in priority order.
        # Both candidates use the SAME joining for old and new so the
        # in-place replacement preserves whatever join character (if
        # any) the matcher used to find it.
        candidates: list[tuple[str, str]] = []
        for joiner in _candidate_joiners(
            edit.context_before, edit.old_string, edit.context_after
        ):
            old_anchor = joiner(edit.context_before, edit.old_string, edit.context_after)
            new_anchor = joiner(edit.context_before, edit.new_string, edit.context_after)
            candidates.append((old_anchor, new_anchor))

        # First pass: any candidate whose old_anchor matches the file?
        for old_anchor, new_anchor in candidates:
            old_count = content.count(old_anchor)
            if old_count == 1:
                return (
                    content.replace(old_anchor, new_anchor, 1),
                    EditResult(status=PatchStatus.APPLIED, matches=1),
                )
            if old_count > 1:
                return content, EditResult(
                    status=PatchStatus.AMBIGUOUS, matches=old_count
                )

        # No old_anchor matched — was the edit already applied?
        for _, new_anchor in candidates:
            new_count = content.count(new_anchor)
            if new_count == 1:
                return content, EditResult(
                    status=PatchStatus.ALREADY_APPLIED, matches=1
                )
            if new_count > 1:
                return content, EditResult(
                    status=PatchStatus.AMBIGUOUS, matches=new_count
                )

        return content, EditResult(status=PatchStatus.NOT_FOUND, matches=0)

    # Unanchored fallback: old_string alone must be unique.
    raw_count = content.count(edit.old_string)
    if raw_count == 1:
        new_content = content.replace(edit.old_string, edit.new_string, 1)
        return new_content, EditResult(status=PatchStatus.APPLIED, matches=1)
    if raw_count > 1:
        return content, EditResult(status=PatchStatus.AMBIGUOUS, matches=raw_count)
    return content, EditResult(status=PatchStatus.NOT_FOUND, matches=0)


def _candidate_joiners(cb: str, mid: str, ca: str):
    """Yield join strategies in priority order: raw first, then
    newline-aware. Returns a list of callables `(cb, mid, ca) -> str`.

    Order matters: raw first means inline anchors like
    `cb=""`, `mid="x=1"`, `ca="\\n"` keep their existing behaviour.
    Only when the raw join yields zero / multiple hits do we try the
    newline-aware join, which is what line-level LLM-emitted anchors
    need.
    """

    def raw(cb_: str, mid_: str, ca_: str) -> str:
        return cb_ + mid_ + ca_

    joiners = [raw]

    # Decide whether a newline-aware variant would actually differ
    # from the raw join. If both contexts are empty, or the boundaries
    # already carry newlines, nothing changes — skip it to keep
    # candidate count minimal.
    cb_needs_nl = bool(cb) and not cb.endswith("\n") and not mid.startswith("\n")
    ca_needs_nl = bool(ca) and not ca.startswith("\n") and not mid.endswith("\n")
    if cb_needs_nl or ca_needs_nl:

        def nl_aware(cb_: str, mid_: str, ca_: str) -> str:
            left = cb_ + "\n" if cb_needs_nl else cb_
            right = "\n" + ca_ if ca_needs_nl else ca_
            return left + mid_ + right

        joiners.append(nl_aware)

    return joiners


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
