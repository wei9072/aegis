"""
Semantic comparison primitives.

A SemanticComparator answers one question: how much do two pieces of
text mean the same thing? It is intentionally minimal — overlap score
+ rationale — so the same engine can serve both ToolCallValidator
Tier-2 (claim vs actual write) and IntentBypassDetector (prompt
rejection-target vs response content). ROADMAP §3.1 / §4.1 require
these two layers to share one comparator; that requirement starts here.

This module exposes the protocol and a stub implementation only. The
real LLM-backed comparator lives next to the provider it depends on.
"""
from aegis.semantic.comparator import (
    SemanticComparator,
    SemanticResult,
    StubSemanticComparator,
)

__all__ = ["SemanticComparator", "SemanticResult", "StubSemanticComparator"]
