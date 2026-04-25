"""
Policy layer — deterministic rule evaluation over DecisionTrace.

Phase 1 contract: the policy engine is the ONLY component that emits
`policy:<verb>` events. Upstream gates (Ring 0.5, ToolCallValidator, ...)
emit raw observations; the engine reads those observations and decides
whether to escalate to `warn` or `block`. No LLM calls, no I/O.
"""
from aegis.policy.engine import (
    DEFAULT_RULES,
    PolicyEngine,
    PolicyVerdict,
    SignalRule,
)

__all__ = [
    "DEFAULT_RULES",
    "PolicyEngine",
    "PolicyVerdict",
    "SignalRule",
]
