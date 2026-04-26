"""
Groq provider — OpenAI-compatible gateway with hardware-accelerated
inference for several open-weight model families (Llama 3/4, Qwen,
gpt-oss, Allam) plus Groq's own compound models.

Aegis uses this as a third cross-model evidence path alongside the
Gemini-native and OpenRouter providers. Groq's free tier offers
generous per-day request budgets across diverse model families,
which is exactly the ingredient missing from the V1 sweep where
gemini-2.5-flash hit a 20-req/day cap.

Default model is `llama-3.3-70b-versatile` — a capable Meta Llama
that's typically more disciplined about anchor formatting than
ling-2.6 (a useful contrast for L5 cross-model evidence).

Authentication is read from `GROQ_API_KEY`. Override at construct
time with `api_key=` if needed.
"""
from __future__ import annotations

from typing import Optional

from aegis.agents.openai import OpenAIProvider


DEFAULT_MODEL = "llama-3.3-70b-versatile"
BASE_URL = "https://api.groq.com/openai/v1"


class GroqProvider(OpenAIProvider):
    def __init__(
        self,
        model_name: str = DEFAULT_MODEL,
        *,
        api_key: Optional[str] = None,
        timeout: int = 120,
    ) -> None:
        super().__init__(
            model_name=model_name,
            api_key=api_key,
            base_url=BASE_URL,
            api_key_env="GROQ_API_KEY",
            timeout=timeout,
        )
