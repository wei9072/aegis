"""
Unit tests for the OpenAI-compatible provider + OpenRouter wrapper.

We never hit a real network in tests — every external call is mocked at
the urllib boundary. The point is to pin:
  - successful response parsing (the standard chat-completion shape)
  - graceful failure when the body shape drifts
  - HTTP error translation (so trace failures carry useful info)
  - mutating-tool rejection (defence-in-depth invariant)
  - OpenRouter wrapper actually overrides base_url and api_key_env
"""
from __future__ import annotations

import io
import json
import urllib.error
from unittest.mock import patch

import pytest

from aegis.agents.openai import OpenAIProvider
from aegis.agents.openrouter import OpenRouterProvider, BASE_URL, DEFAULT_MODEL


class _FakeResponse:
    """urllib.urlopen context-manager stand-in."""

    def __init__(self, body: bytes):
        self._body = body

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

    def read(self):
        return self._body


def _ok_response(content: str) -> _FakeResponse:
    return _FakeResponse(json.dumps({
        "choices": [{"message": {"content": content}}],
    }).encode("utf-8"))


def test_provider_returns_assistant_content():
    with patch("urllib.request.urlopen", return_value=_ok_response("hello")):
        p = OpenAIProvider(model_name="x", api_key="dummy")
        assert p.generate("hi") == "hello"


def test_provider_records_tools_for_observation():
    """last_used_tools must match what was passed in (Gateway reads this
    to emit the provider:tool_surface trace event)."""
    with patch("urllib.request.urlopen", return_value=_ok_response("ok")):
        p = OpenAIProvider(model_name="x", api_key="dummy")
        from aegis.tools.file_system import read_file, list_directory
        p.generate("hi", tools=(read_file, list_directory))
        assert p.last_used_tools == (read_file, list_directory)


def test_provider_rejects_mutating_tools():
    """Defence-in-depth: even though tools currently aren't forwarded,
    refuse on construct so a future tool-dispatch path can't quietly
    leak a writer."""
    p = OpenAIProvider(model_name="x", api_key="dummy")

    def write_file(path, content):
        return None
    write_file.__name__ = "write_file"

    with pytest.raises(ValueError, match="state-mutating"):
        p.generate("hi", tools=(write_file,))


def test_provider_translates_http_error():
    err = urllib.error.HTTPError(
        url="x", code=429, msg="Too Many Requests",
        hdrs=None, fp=io.BytesIO(b'{"error":"rate limited"}'),
    )
    with patch("urllib.request.urlopen", side_effect=err):
        p = OpenAIProvider(model_name="x", api_key="dummy")
        with pytest.raises(RuntimeError, match="HTTP 429"):
            p.generate("hi")


def test_provider_translates_url_error():
    with patch("urllib.request.urlopen",
               side_effect=urllib.error.URLError("connection refused")):
        p = OpenAIProvider(model_name="x", api_key="dummy")
        with pytest.raises(RuntimeError, match="request to .* failed"):
            p.generate("hi")


def test_provider_raises_on_malformed_json():
    with patch("urllib.request.urlopen",
               return_value=_FakeResponse(b"<html>not json</html>")):
        p = OpenAIProvider(model_name="x", api_key="dummy")
        with pytest.raises(RuntimeError, match="Unexpected response shape"):
            p.generate("hi")


def test_provider_raises_on_missing_choices():
    """Body parses as JSON but lacks the choices/[0]/message/content path."""
    body = json.dumps({"object": "error", "message": "auth"}).encode("utf-8")
    with patch("urllib.request.urlopen", return_value=_FakeResponse(body)):
        p = OpenAIProvider(model_name="x", api_key="dummy")
        with pytest.raises(RuntimeError, match="Unexpected response shape"):
            p.generate("hi")


def test_provider_uses_explicit_api_key_over_env(monkeypatch):
    """Explicit api_key= argument wins over env-var lookup."""
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    p = OpenAIProvider(model_name="x", api_key="explicit-key")
    assert p.api_key == "explicit-key"


def test_provider_falls_back_to_env(monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "from-env")
    p = OpenAIProvider(model_name="x")
    assert p.api_key == "from-env"


def test_provider_raises_when_no_credential(monkeypatch):
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    with pytest.raises(ValueError, match="OPENAI_API_KEY is not set"):
        OpenAIProvider(model_name="x")


def test_openrouter_overrides_base_url_and_env(monkeypatch):
    """The whole point of the subclass: hit a different endpoint with a
    different env var, but keep all the parsing / error-translation
    logic of the base class."""
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.setenv("OPENROUTER_API_KEY", "router-key")
    p = OpenRouterProvider()  # default model
    assert p.base_url == BASE_URL
    assert p.api_key == "router-key"
    assert p.model_name == DEFAULT_MODEL


def test_openrouter_default_model_is_pinned():
    """Renaming this constant is a breaking change for downstream tooling
    that defaults to it — pin it explicitly."""
    assert DEFAULT_MODEL == "inclusionai/ling-2.6-1t:free"
    assert BASE_URL == "https://openrouter.ai/api/v1"


def test_openrouter_carries_request_body_to_target_url(monkeypatch):
    """End-to-end smoke: when generate() runs, the resulting request
    points at OpenRouter's URL and carries the model id through."""
    monkeypatch.setenv("OPENROUTER_API_KEY", "k")
    captured = {}

    class _Capture:
        def __enter__(self_inner):
            return self_inner
        def __exit__(self_inner, *exc):
            return False
        def read(self_inner):
            return _ok_response("done").read()

    def fake_urlopen(req, timeout):
        captured["url"] = req.full_url
        captured["body"] = json.loads(req.data.decode("utf-8"))
        captured["auth"] = req.get_header("Authorization")
        return _Capture()

    with patch("urllib.request.urlopen", side_effect=fake_urlopen):
        OpenRouterProvider().generate("ping")

    assert captured["url"] == f"{BASE_URL}/chat/completions"
    assert captured["body"]["model"] == DEFAULT_MODEL
    assert captured["body"]["messages"] == [{"role": "user", "content": "ping"}]
    assert captured["auth"] == "Bearer k"
