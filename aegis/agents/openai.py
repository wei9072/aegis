"""
OpenAI provider stub. Implement when adding OpenAI support.
"""
from aegis.agents.llm_adapter import LLMProvider


class OpenAIProvider(LLMProvider):
    last_used_tools: tuple = ()

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        raise NotImplementedError("OpenAI provider — implement with openai SDK")
