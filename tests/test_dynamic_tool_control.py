"""
Dynamic tool control: tool surface is per-request, not per-session.

These tests verify the precondition for a future intent-classification
layer that will vary the LLM's capability per request. The layer itself
doesn't exist yet — what matters now is that the plumbing accepts a
`tools` parameter at the gateway, threads it through to the provider,
and emits an OBSERVE event so the eval harness can see which tools were
active for each call.
"""
import pytest

from aegis.agents.gemini import LLM_TOOLS_READ_ONLY, _validate_tool_surface
from aegis.agents.llm_adapter import LLMGateway
from aegis.runtime.trace import OBSERVE
from aegis.tools.file_system import (
    MUTATING_TOOL_NAMES,
    list_directory,
    read_file,
    write_file,
)


# ---------- Recording fake provider ----------

class _RecordingProvider:
    """Captures every (prompt, tools) the gateway hands it."""
    last_used_tools: tuple = ()

    def __init__(self, response: str = "x = 1"):
        self.calls: list[tuple[str, tuple]] = []
        self._response = response

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        active = tuple(tools) if tools is not None else ()
        self.calls.append((prompt, active))
        self.last_used_tools = active
        return self._response


# ---------- Runtime tool-surface validator ----------

def test_validator_accepts_read_only_tools():
    _validate_tool_surface((read_file, list_directory))


def test_validator_rejects_mutating_tool():
    with pytest.raises(ValueError, match="write_file"):
        _validate_tool_surface((read_file, write_file))


def test_validator_rejection_mentions_executor():
    with pytest.raises(ValueError, match="Executor"):
        _validate_tool_surface((write_file,))


def test_mutating_set_matches_validator():
    """The set used by the structural test and the runtime validator
    must stay in sync — otherwise a future addition could be guarded
    in one place but not the other."""
    assert "write_file" in MUTATING_TOOL_NAMES


# ---------- Default tool surface ----------

def test_default_tools_are_read_only_constants():
    names = {t.__name__ for t in LLM_TOOLS_READ_ONLY}
    assert names == {"read_file", "list_directory"}


# ---------- Gateway → provider tool plumbing ----------

def test_gateway_passes_no_tools_by_default():
    """Gateway with no `tools` argument lets the provider fall back to
    its own default — the gateway does not silently inject a tool list."""
    provider = _RecordingProvider()
    LLMGateway(llm_provider=provider).generate_and_validate("hi")

    assert len(provider.calls) == 1
    _, used = provider.calls[0]
    assert used == ()  # provider received tools=None → recorded as ()


def test_gateway_propagates_explicit_tools():
    provider = _RecordingProvider()
    custom = (read_file,)
    LLMGateway(llm_provider=provider).generate_and_validate("hi", tools=custom)

    _, used = provider.calls[0]
    assert used == custom


def test_gateway_passes_same_tools_on_each_retry():
    provider = _RecordingProvider(response="```python\ndef bad(\n```")
    custom = (read_file, list_directory)
    with pytest.raises(RuntimeError):
        LLMGateway(llm_provider=provider).generate_and_validate(
            "force-retry", max_retries=3, tools=custom
        )

    assert len(provider.calls) == 3
    assert all(used == custom for _, used in provider.calls)


# ---------- Trace records the resolved tool surface ----------

def test_trace_records_tool_surface_per_attempt_on_success():
    provider = _RecordingProvider()
    gw = LLMGateway(llm_provider=provider)
    gw.generate_and_validate("hi", tools=(read_file,))

    surfaces = [
        e for e in gw.last_trace.by_layer("provider")
        if e.reason == "tool_surface"
    ]
    assert len(surfaces) == 1
    assert surfaces[0].metadata["tools"] == ["read_file"]
    assert surfaces[0].metadata["attempt"] == 1


def test_trace_records_tool_surface_per_attempt_across_retries():
    provider = _RecordingProvider(response="```python\ndef bad(\n```")
    gw = LLMGateway(llm_provider=provider)
    with pytest.raises(RuntimeError):
        gw.generate_and_validate("force-retry", max_retries=3, tools=(read_file,))

    surfaces = [
        e for e in gw.last_trace.by_layer("provider")
        if e.reason == "tool_surface"
    ]
    assert len(surfaces) == 3
    assert [s.metadata["attempt"] for s in surfaces] == [1, 2, 3]


def test_trace_records_empty_tools_when_provider_uses_default():
    """When no tools are passed, provider falls back to its own default;
    the trace records what the provider actually used (empty for fakes
    that don't track resolution, populated for real providers)."""
    provider = _RecordingProvider()
    gw = LLMGateway(llm_provider=provider)
    gw.generate_and_validate("hi")

    surface = gw.last_trace.by_layer("provider")[0]
    assert surface.decision == OBSERVE
    assert surface.reason == "tool_surface"
    assert surface.metadata["tools"] == []


# ---------- GeminiProvider runtime validation (no API call) ----------

def test_gemini_provider_rejects_mutating_tool_before_api_call():
    """Validation fires before any chat session is created, so this test
    needs no mock — instantiation does require an API key, which we pass
    explicitly to avoid touching the environment."""
    from aegis.agents.gemini import GeminiProvider

    provider = GeminiProvider(api_key="test-key-not-real")
    with pytest.raises(ValueError, match="write_file"):
        provider.generate("hi", tools=(write_file,))


def test_gemini_provider_no_session_attribute():
    """The cached `_chat_session` attribute is gone — sessions are now
    created per request so a future intent layer can vary tools."""
    from aegis.agents.gemini import GeminiProvider

    provider = GeminiProvider(api_key="test-key-not-real")
    assert not hasattr(provider, "_chat_session"), (
        "GeminiProvider still caches a chat session — tool list would be "
        "locked at session creation, defeating dynamic tool control."
    )


def test_gemini_provider_records_last_used_tools():
    from aegis.agents.gemini import GeminiProvider

    provider = GeminiProvider(api_key="test-key-not-real")
    assert provider.last_used_tools == ()
    # Even a rejected call should not silently update last_used_tools
    with pytest.raises(ValueError):
        provider.generate("hi", tools=(write_file,))
    assert provider.last_used_tools == ()
