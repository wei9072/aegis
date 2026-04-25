"""
Intent layer — front-gate classifier that labels the incoming prompt.

The intent label is a *presentation hint*, never an enforcement lever.
It MUST NOT loosen any invariant: a `teaching` prompt that triggers
fan_out=15 still gets `policy:warn` exactly like a `normal_dev` one.
The label exists so downstream layers (Phase 3 intent-bypass) have a
baseline to compare the response against.

Tier-1 (this module) is deterministic keyword/phrase matching — 0
token, fail-open to NORMAL_DEV when uncertain. A future Tier-2 may
add an LLM classifier; the public API will stay the same.
"""
from aegis.intent.bypass import BypassVerdict, IntentBypassDetector
from aegis.intent.classifier import Intent, IntentClassifier

__all__ = [
    "BypassVerdict",
    "Intent",
    "IntentBypassDetector",
    "IntentClassifier",
]
