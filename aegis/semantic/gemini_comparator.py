"""
GeminiSemanticComparator — production SemanticComparator backed by Gemini.

The module is import-safe without an API key: the underlying provider
is created lazily on the first `compare()` call. Tests that want to
exercise the parser without a network round-trip can inject a fake
provider via the `provider` constructor argument.

Prompt design:
  - The caller supplies a free-form `context` string that frames what
    "satisfy" means for this comparison (intent-bypass vs Tier-2
    claim verification share this engine via different framings).
  - The model is forced to emit a single-line JSON object so parsing
    stays deterministic. If parsing fails we return overlap=0.0 with
    a rationale that pinpoints the failure — fail-open is safer than
    a phantom block.
"""
from __future__ import annotations

import json
import re
from typing import Optional

from aegis.semantic.comparator import SemanticResult

# Avoid `from aegis.agents.gemini import GeminiProvider` at module load
# time — that path drags in aegis.agents.llm_adapter, which imports
# aegis.intent.bypass, which imports aegis.semantic.*, which is *us*.
# The provider is instantiated lazily inside `provider` instead.


_COMPARE_PROMPT = """You are a deterministic semantic-comparison oracle.

Given two pieces of text labeled A and B, decide how strongly B \
semantically satisfies A within the supplied context. Output ONLY a \
single-line JSON object — no prose, no fences:

{{"overlap": <float in [0.0, 1.0]>, "rationale": "<one short sentence>"}}

Scale:
  0.0  B does not satisfy A at all.
  0.5  B partially satisfies A or is ambiguous.
  1.0  B clearly and completely satisfies A.

Context: {context}

A:
{a}

B:
{b}
"""


_JSON_RE = re.compile(r"\{[^{}]*\"overlap\"[^{}]*\}", re.DOTALL)


class GeminiSemanticComparator:
    """LLM-backed comparator. One Gemini call per `compare()`."""

    def __init__(
        self,
        provider: Optional[object] = None,
        *,
        model_name: str = "gemini-2.5-flash",
    ) -> None:
        # Lazy: defer GeminiProvider() until compare() so importing
        # this module never requires GEMINI_API_KEY at process start.
        self._provider = provider
        self._model_name = model_name

    @property
    def provider(self):
        if self._provider is None:
            from aegis.agents.gemini import GeminiProvider
            self._provider = GeminiProvider(model_name=self._model_name)
        return self._provider

    def compare(self, a: str, b: str, *, context: str = "") -> SemanticResult:
        prompt = _COMPARE_PROMPT.format(
            a=a,
            b=b,
            context=context or "general semantic match",
        )
        # Empty tool surface — the comparator is pure text reasoning,
        # giving the LLM tools here would only invite distraction.
        raw = self.provider.generate(prompt, tools=())
        return self._parse(raw)

    @staticmethod
    def _parse(raw: str) -> SemanticResult:
        if not raw:
            return SemanticResult(overlap=0.0, rationale="empty response")
        match = _JSON_RE.search(raw)
        if match is None:
            return SemanticResult(
                overlap=0.0,
                rationale=f"unparseable: {raw[:80]!r}",
            )
        try:
            data = json.loads(match.group(0))
        except json.JSONDecodeError as e:
            return SemanticResult(
                overlap=0.0,
                rationale=f"json error: {e}",
            )
        try:
            overlap = float(data.get("overlap", 0.0))
        except (TypeError, ValueError):
            return SemanticResult(
                overlap=0.0,
                rationale="overlap not a number",
            )
        # Clamp into [0, 1] — defends against an LLM that drifts past
        # the documented scale (1.5 / -0.2 / etc.).
        overlap = max(0.0, min(1.0, overlap))
        rationale = str(data.get("rationale", "")).strip()
        return SemanticResult(overlap=overlap, rationale=rationale)
