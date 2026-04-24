import os
from google import genai
from google.genai import types
from aegis.agents.llm_adapter import LLMProvider
from aegis.tools.file_system import read_file, write_file, list_directory


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
                        tools=[read_file, write_file, list_directory]
                    )
                )
            response = self._chat_session.send_message(prompt)
            return response.text or ""
        except Exception as e:
            raise Exception(f"Failed to generate content from {self.model_name}: {e}")
