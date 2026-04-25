"""
Gemini provider.

Tool surface policy: the LLM is given READ-ONLY tools by default. Any
state-mutating operation (writing files, creating directories, deleting
paths, executing shell) must go through aegis.runtime.executor.Executor,
which provides backups, atomic apply, rollback, and emits DecisionTrace
events.

This provider is also stateless across requests by design: each call to
`generate()` creates a fresh chat session with a freshly-resolved tool
surface. Without this, tool selection is locked at session creation and
a future intent layer cannot vary the LLM's capability per request.
Conversation continuity, if needed, must be reintroduced explicitly
(e.g. by passing prior turns as part of `prompt`), not implicitly via
session state.
"""
import os

from google import genai
from google.genai import types

from aegis.agents.llm_adapter import LLMProvider
from aegis.tools.file_system import MUTATING_TOOL_NAMES, read_file, list_directory


# Default read-only tool surface. Adding a write here would create a
# second source of truth for filesystem state and break the precondition
# for ToolCallValidator. The runtime validator below also defends against
# callers passing a mutating tool through the `tools` parameter.
LLM_TOOLS_READ_ONLY = (read_file, list_directory)


def _validate_tool_surface(tools) -> None:
    """Reject any mutating callable before it reaches the model.

    Defence-in-depth on top of the structural guards in
    tests/test_side_effect_isolation.py: even if a future caller imports a
    mutating helper directly and passes it as a tool, this check fails
    fast before the API request is built.
    """
    for tool in tools:
        name = getattr(tool, "__name__", "")
        if name in MUTATING_TOOL_NAMES:
            raise ValueError(
                f"Tool '{name}' is a state-mutating callable and cannot be "
                f"exposed to the LLM. Route writes through "
                f"aegis.runtime.executor.Executor instead."
            )


class GeminiProvider(LLMProvider):
    def __init__(self, model_name: str = "gemini-2.5-flash", api_key: str = None):
        key = api_key or os.environ.get("GEMINI_API_KEY")
        if not key:
            raise ValueError("GEMINI_API_KEY is not set.")
        self.client = genai.Client(api_key=key)
        self.model_name = model_name
        # Last-resolved tool surface for the most recent generate() call.
        # The Gateway reads this to record an OBSERVE event into the trace.
        self.last_used_tools: tuple = ()

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        active_tools = tuple(tools) if tools is not None else LLM_TOOLS_READ_ONLY
        _validate_tool_surface(active_tools)
        self.last_used_tools = active_tools

        try:
            chat = self.client.chats.create(
                model=self.model_name,
                config=types.GenerateContentConfig(tools=list(active_tools)),
            )
            response = chat.send_message(prompt)
            return response.text or ""
        except Exception as e:
            raise Exception(f"Failed to generate content from {self.model_name}: {e}")
