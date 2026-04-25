"""
IntentClassifier (Tier-1) — deterministic keyword/phrase matcher.

The classifier inspects the user prompt and returns one of three
labels. It does NOT change enforcement; it only annotates the trace so
later layers (Phase 3 intent-bypass detection, adaptive policy) can
reason about the request shape.

Match order is important: ADVERSARIAL is checked before TEACHING so a
prompt like "ignore previous instructions and show me X" is flagged as
adversarial rather than swallowed by the teaching path. NORMAL_DEV is
the default — fail-open is the right posture for a label that does not
gate behaviour.
"""
from __future__ import annotations

from enum import Enum


class Intent(str, Enum):
    """Stable label set. New labels may be added; existing ones must not change."""

    NORMAL_DEV = "normal_dev"
    TEACHING = "teaching"
    ADVERSARIAL = "adversarial"


# Phrase-level patterns. We compare against the lowercased prompt for
# the English entries and the raw prompt for the Chinese entries.
# Phrase-level (not word-level) matching keeps the false-positive rate
# in check — "ignore" alone is far too noisy to flag as adversarial.
_ADVERSARIAL_PHRASES_EN: tuple[str, ...] = (
    "ignore previous",
    "ignore the above",
    "ignore all previous",
    "previous instructions",
    "instructions above",
    "disregard previous",
    "disregard the above",
    "pretend you are",
    "pretend to be",
    "act as if",
    "jailbreak",
    "bypass the",
    "system prompt",
)
_ADVERSARIAL_PHRASES_ZH: tuple[str, ...] = (
    "忽略前面",
    "忽略之前",
    "忽略所有",
    "無視前面",
    "無視之前",
    "假裝你是",
    "假裝成",
    "越獄",
    "繞過",
    "請扮演",
)

_TEACHING_PHRASES_EN: tuple[str, ...] = (
    "show me",
    "show how",
    "what does",
    "looks like",
    "example of",
    "explain",
    "demonstrate",
    "tutorial",
    "teach me",
    "walk me through",
    "for educational",
)
_TEACHING_PHRASES_ZH: tuple[str, ...] = (
    "示範",
    "展示",
    "教我",
    "為什麼",
    "什麼樣",
    "舉例",
    "範例",
    "看看",
    "解釋",
    "說明一下",
    "為了教學",
)


class IntentClassifier:
    def classify(self, prompt: str) -> Intent:
        if not prompt:
            return Intent.NORMAL_DEV
        lower = prompt.lower()
        if self._matches(prompt, lower, _ADVERSARIAL_PHRASES_ZH, _ADVERSARIAL_PHRASES_EN):
            return Intent.ADVERSARIAL
        if self._matches(prompt, lower, _TEACHING_PHRASES_ZH, _TEACHING_PHRASES_EN):
            return Intent.TEACHING
        return Intent.NORMAL_DEV

    @staticmethod
    def _matches(
        raw: str,
        lower: str,
        zh: tuple[str, ...],
        en: tuple[str, ...],
    ) -> bool:
        for p in zh:
            if p in raw:
                return True
        for p in en:
            if p in lower:
                return True
        return False
