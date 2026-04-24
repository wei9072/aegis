"""
OpenAI provider stub. Implement when adding OpenAI support.
"""
from aegis.agents.llm_adapter import LLMProvider


class OpenAIProvider(LLMProvider):
    def generate(self, prompt: str) -> str:
        raise NotImplementedError("OpenAI provider — implement with openai SDK")
