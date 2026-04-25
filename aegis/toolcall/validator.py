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
from aegis.runtime.trace import BLOCK, PASS, DecisionEvent, DecisionTrace
from aegis.semantic.comparator import SemanticComparator


# Verbs that signal "I performed a side effect on the filesystem".
# Ordered loosely by frequency in real LLM output; order is irrelevant
# at runtime since we use `any(... in text)`.
_WRITE_VERBS_ZH: tuple[str, ...] = (
    # Order doesn't matter at runtime, but "寫" must stay in the list
    # for narrations like "我為你寫了 X.py" — narrower verbs ("寫入"
    # alone) miss the bare-write phrasing that real LLMs prefer.
    "寫", "創建", "建立", "新建", "建造", "儲存", "存入", "生成", "產生",
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
    def __init__(
        self,
        comparator: SemanticComparator | None = None,
        *,
        threshold: float = 0.7,
    ) -> None:
        # When `comparator` is None, only Tier-1 (deterministic
        # path-existence checks) runs. Wiring a comparator turns on
        # Tier-2 (semantic claim/content comparison) — one extra LLM
        # call per turn that survives Tier-1 with executor activity.
        self.comparator = comparator
        self.threshold = threshold

    def validate(
        self,
        response_text: str,
        executor_result: ExecutionResult | None,
        trace: DecisionTrace | None = None,
    ) -> ToolCallVerdict:
        """Compare narrated claims against actual executor activity.

        Two-tier shape (ROADMAP §3.1 / §4.1):
          - Tier-1: deterministic, no LLM. Always runs. Catches the
            common hallucination shapes.
          - Tier-2: semantic, one LLM call. Runs only when Tier-1
            cleared, executor wrote *something*, and a comparator was
            wired in. Compares LLM narration against actual content.

        `executor_result=None` is treated as "no executor was wired in
        for this turn" — equivalent to an empty ExecutionResult.
        """
        verdict = ToolCallVerdict()
        claimed, paths = claim_implies_write(response_text)
        if not claimed:
            return verdict

        touched = self._touched_paths(executor_result)
        # Tier-1 shape #1: claimed paths but executor recorded zero
        # writes. Catches scenario 10 (pure-text claim, no tool call
        # attempted at all).
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

        # Tier-1 shape #2: executor wrote *something* but none of the
        # claimed paths match. The mismatch is still deterministic —
        # exact path comparison, no semantics.
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

        # Tier-1 cleared. Run Tier-2 if a comparator is wired in and
        # we actually have content to compare against.
        if self.comparator is None or executor_result is None:
            return verdict

        matched_contents = self._collect_matched_contents(
            paths, touched, executor_result.path_contents,
        )
        if not matched_contents:
            return verdict

        event = self._tier2_compare(response_text, matched_contents, trace)
        if event is not None:
            verdict.events.append(event)
        return verdict

    def _tier2_compare(
        self,
        response_text: str,
        matched_contents: list[tuple[str, str]],
        trace: DecisionTrace | None,
    ) -> DecisionEvent | None:
        # One comparison call per turn: concatenate every matched
        # path's actual content and compare against the LLM narration
        # in a single round-trip. Multiple per-path calls would
        # multiply LLM cost without proportionally better signal.
        combined = "\n\n".join(f"--- {p} ---\n{c}" for p, c in matched_contents)
        context = (
            "tool_call_tier2: A is the LLM's natural-language narration "
            "of what it just wrote. B is the actual file content the "
            "executor produced. overlap=1.0 means narration faithfully "
            "describes content; 0.0 means they describe different things "
            "(hallucinated description of an unrelated write)."
        )
        result = self.comparator.compare(response_text, combined, context=context)
        # Invariant 4: emit on every executed run, block or pass alike.
        if trace is None:
            return None
        block = result.overlap < self.threshold
        return trace.emit(
            layer="toolcall",
            decision=BLOCK if block else PASS,
            reason="claim_content_mismatch" if block else "claim_content_matches",
            metadata={
                "matched_paths": [p for p, _ in matched_contents],
                "overlap": result.overlap,
                "threshold": self.threshold,
                "rationale": result.rationale,
            },
        )

    @staticmethod
    def _collect_matched_contents(
        claimed_paths: list[str],
        touched: list[str],
        path_contents: dict[str, str],
    ) -> list[tuple[str, str]]:
        """Pick (touched_path, content) pairs that match any claimed path.

        Reuses the same suffix-tolerant comparison Tier-1 uses, so
        there is exactly one notion of "this claim refers to that
        write" across both tiers.
        """
        out: list[tuple[str, str]] = []
        for t in touched:
            if t not in path_contents:
                continue
            for c in claimed_paths:
                if ToolCallValidator._path_matches_any(c, [t]):
                    out.append((t, path_contents[t]))
                    break
        return out

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
