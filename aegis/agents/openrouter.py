"""
OpenRouter provider — OpenAI-compatible gateway over many model backends.

OpenRouter exposes a unified `/v1/chat/completions` surface in front of
Anthropic / Mistral / Inclusion AI / Google / DeepSeek / etc. Aegis uses
this as the cheap path to the V1 charter's L5 layer (cross-model
validation): swap `--provider openrouter --model <slug>` and the same
multi-turn scenario runs against a different model family, no extra
SDK needed.

Default model is `inclusionai/ling-2.6-1t:free`. Free-tier models have
rate limits and per-day token ceilings; suitable for dogfood probes and
L5 spot checks, not for high-volume production traffic.

Authentication is read from `OPENROUTER_API_KEY`. Override at construct
time with `api_key=` if needed.
"""
from __future__ import annotations

from typing import Optional

from aegis.agents.openai import OpenAIProvider


DEFAULT_MODEL = "inclusionai/ling-2.6-1t:free"
BASE_URL = "https://openrouter.ai/api/v1"


class OpenRouterProvider(OpenAIProvider):
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
            api_key_env="OPENROUTER_API_KEY",
            timeout=timeout,
        )
