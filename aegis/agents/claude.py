"""
Claude (Anthropic) provider stub. Implement when adding Claude support.
"""
from aegis.agents.llm_adapter import LLMProvider


class ClaudeProvider(LLMProvider):
    last_used_tools: tuple = ()

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        raise NotImplementedError("Claude provider — implement with anthropic SDK")
