"""
ToolCallValidator (Tier-1) — deterministic hallucination guard.

The validator inspects the LLM's full response text for natural-language
claims that it wrote / created / saved a file, extracts the path-like
tokens it mentioned, and cross-checks against the ExecutionResult the
caller supplies. If the model claimed a write but the executor never
touched the matching path, the claim is hallucinated and we emit
`toolcall:block hallucinated_claim_no_write`.

Design notes:
  - Tier-1 is deterministic: regex over both Chinese and English write
    verbs, plus a file-extension token match. False positives are
    preferable to false negatives here — a bogus block costs one retry,
    a missed hallucination silently misleads the user.
  - The validator never reads the live filesystem; ExecutionResult is
    the single source of truth about what actually happened. This
    upholds invariant 6 (decision phase reads only the caller-provided
    snapshot, not live state).
  - This file has no path-escape / sandbox enforcement yet — that's a
    separate concern handled by the Executor when patches reach disk.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field

from aegis.runtime.executor import ExecutionResult
from aegis.runtime.trace import BLOCK, DecisionEvent, DecisionTrace


# Verbs that signal "I performed a side effect on the filesystem".
# Ordered loosely by frequency in real LLM output; order is irrelevant
# at runtime since we use `any(... in text)`.
_WRITE_VERBS_ZH: tuple[str, ...] = (
    "創建", "建立", "新建", "建造", "寫入", "儲存", "存入", "生成", "產生",
)
_WRITE_VERBS_EN: tuple[str, ...] = (
    "created", "wrote", "saved", "generated", "added", "produced", "made",
)

# Path-like token: a filename with a recognised source/data extension.
# We deliberately keep this narrow to avoid matching prose like "v1.0"
# or "section 3.2" — only real file shapes count as "claimed paths".
_PATH_RE = re.compile(
    r"\b[\w./\-]+?\.(?:py|md|txt|json|yaml|yml|toml|js|ts|tsx|html|css|sh|rs|go|java)\b",
    re.IGNORECASE,
)


def claim_implies_write(text: str) -> tuple[bool, list[str]]:
    """Detect a self-narrated write claim in `text`.

    Returns `(claimed, paths)`:
      - `claimed` is True iff at least one write verb AND at least one
        path-like token appear in the same response.
      - `paths` lists the matched path tokens in order of appearance.

    Both conditions are required: "I created a function" alone (no
    file mentioned) is fine; "see fibonacci.py" alone (no claim of
    write) is also fine. Only the conjunction triggers Tier-1.
    """
    if not text:
        return False, []
    has_verb = any(v in text for v in _WRITE_VERBS_ZH) or any(
        v in text.lower() for v in _WRITE_VERBS_EN
    )
    if not has_verb:
        return False, []
    paths = [m.group(0) for m in _PATH_RE.finditer(text)]
    if not paths:
        return False, []
    # Dedupe while preserving order.
    seen: set[str] = set()
    unique = [p for p in paths if not (p in seen or seen.add(p))]
    return True, unique


@dataclass
class ToolCallVerdict:
    """Concrete events the validator appended to the trace this pass.

    Mirrors PolicyVerdict's shape so LLMGateway can check `has_block()`
    without re-scanning the trace.
    """

    events: list[DecisionEvent] = field(default_factory=list)

    def has_block(self) -> bool:
        return any(e.decision == BLOCK for e in self.events)


class ToolCallValidator:
    def validate(
        self,
        response_text: str,
        executor_result: ExecutionResult | None,
        trace: DecisionTrace | None = None,
    ) -> ToolCallVerdict:
        """Compare narrated claims against actual executor activity.

        `executor_result=None` is treated as "no executor was wired in
        for this turn" — equivalent to an empty ExecutionResult. A
        missing executor is itself evidence the LLM cannot have written
        anything, so claims still get blocked.
        """
        verdict = ToolCallVerdict()
        claimed, paths = claim_implies_write(response_text)
        if not claimed:
            return verdict

        touched = self._touched_paths(executor_result)
        # Hallucination shape #1: claimed paths but executor recorded
        # zero writes. Catches scenario 10 (pure-text claim, no tool
        # call attempted at all).
        if not touched:
            event = self._emit_block(
                trace,
                reason="hallucinated_claim_no_write",
                metadata={
                    "claimed_paths": paths,
                    "touched_paths": [],
                },
            )
            if event is not None:
                verdict.events.append(event)
            return verdict

        # Hallucination shape #2: executor wrote *something* but none of
        # the claimed paths match. The mismatch is still deterministic
        # at Tier-1 — exact path comparison, no semantics.
        unmatched = [p for p in paths if not self._path_matches_any(p, touched)]
        if unmatched:
            event = self._emit_block(
                trace,
                reason="claimed_paths_not_written",
                metadata={
                    "claimed_paths": paths,
                    "unmatched_paths": unmatched,
                    "touched_paths": list(touched),
                },
            )
            if event is not None:
                verdict.events.append(event)
        return verdict

    @staticmethod
    def _touched_paths(executor_result: ExecutionResult | None) -> list[str]:
        if executor_result is None:
            return []
        return list(executor_result.touched_paths)

    @staticmethod
    def _path_matches_any(claimed: str, touched: list[str]) -> bool:
        """Suffix-tolerant comparison.

        The LLM often says "fibonacci.py" while Executor records the
        relative path "src/fibonacci.py". Tier-1 accepts a match when
        either string is a path-suffix of the other.
        """
        c = claimed.lstrip("./")
        for t in touched:
            t_norm = t.lstrip("./")
            if c == t_norm or t_norm.endswith("/" + c) or c.endswith("/" + t_norm):
                return True
        return False

    @staticmethod
    def _emit_block(
        trace: DecisionTrace | None,
        *,
        reason: str,
        metadata: dict,
    ) -> DecisionEvent | None:
        if trace is None:
            return None
        return trace.emit(
            layer="toolcall",
            decision=BLOCK,
            reason=reason,
            metadata=metadata,
        )
