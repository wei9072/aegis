"""
Claude (Anthropic) provider stub. Implement when adding Claude support.
"""
from aegis.agents.llm_adapter import LLMProvider


class ClaudeProvider(LLMProvider):
    def generate(self, prompt: str) -> str:
        raise NotImplementedError("Claude provider — implement with anthropic SDK")
