"""
ToolCallValidator — compares LLM textual claims against actual Executor
activity, so the gap between "what the LLM said it did" and "what
Executor actually did" becomes a first-class decision event.

Tier-1 (this module) is deterministic and 0-token: regex-based detection
of write claims plus a path-existence cross-check. It catches the most
common hallucination shape — confidently narrating side effects that
never happened — without invoking another LLM.

Tier-2 (Phase 3) will add semantic comparison and share its comparator
engine with intent-bypass detection.
"""
from aegis.toolcall.validator import (
    ToolCallValidator,
    ToolCallVerdict,
    claim_implies_write,
)

__all__ = [
    "ToolCallValidator",
    "ToolCallVerdict",
    "claim_implies_write",
]
