"""
Gemini provider.

Tool surface policy: the LLM is given READ-ONLY tools only. Any state-mutating
operation (writing files, creating directories, deleting paths, executing
shell) must go through aegis.runtime.executor.Executor, which provides
backups, atomic apply, rollback, and emits DecisionTrace events.

Never add a mutating callable to LLM_TOOLS_READ_ONLY. If a future intent
classification policy needs to grant write access, the correct path is:
  1. Wrap the write through Executor.
  2. Add the wrapper to a separate tool tier (e.g. LLM_TOOLS_EXECUTOR_PROXY).
  3. Switch tiers per request — never broaden this constant.
"""
import os

from google import genai
from google.genai import types

from aegis.agents.llm_adapter import LLMProvider
from aegis.tools.file_system import read_file, list_directory


# Read-only tool surface exposed to Gemini. Adding a write here would
# create a second source of truth for filesystem state and break the
# precondition for ToolCallValidator.
LLM_TOOLS_READ_ONLY = (read_file, list_directory)


class GeminiProvider(LLMProvider):
    def __init__(self, model_name: str = "gemini-2.5-flash", api_key: str = None):
        key = api_key or os.environ.get("GEMINI_API_KEY")
        if not key:
            raise ValueError("GEMINI_API_KEY is not set.")
        self.client = genai.Client(api_key=key)
        self.model_name = model_name
        self._chat_session = None

    def generate(self, prompt: str) -> str:
        try:
            if not self._chat_session:
                self._chat_session = self.client.chats.create(
                    model=self.model_name,
                    config=types.GenerateContentConfig(
                        tools=list(LLM_TOOLS_READ_ONLY),
                    ),
                )
            response = self._chat_session.send_message(prompt)
            return response.text or ""
        except Exception as e:
            raise Exception(f"Failed to generate content from {self.model_name}: {e}")
