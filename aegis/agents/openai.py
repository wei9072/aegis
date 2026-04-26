"""
OpenAI-compatible chat completions provider.

Works with any service implementing the OpenAI `/v1/chat/completions`
contract — the actual OpenAI API, OpenRouter, Together, Groq, Anyscale,
Ollama (with the OpenAI-compat shim), vLLM-served endpoints, etc.
Subclasses or callers configure the endpoint via `base_url` plus the
env-var name to read credentials from.

Tool calling: not wired into this base. The multi-turn refactor pipeline
does its file inspection through prompt context (`file_snippets` in
`PlanContext`), not via runtime tool calls, so this provider already
serves the pipeline's needs without a tool dispatch surface. Calls that
pass mutating tools are rejected (defence-in-depth consistent with
GeminiProvider). Read-only tools are recorded but currently not
forwarded to the model — add OpenAI-style tool specs here if and when
tool dispatch is needed.

Stdlib-only HTTP via urllib so we don't pull in `openai` or `requests`
just for a single POST. Failure modes (HTTPError, URLError, malformed
JSON) translate to RuntimeError with the response body excerpt so the
trace shows what actually came back.
"""
from __future__ import annotations

import concurrent.futures
import json
import os
import urllib.error
import urllib.request
from typing import Optional

from aegis.agents.llm_adapter import LLMProvider
from aegis.tools.file_system import MUTATING_TOOL_NAMES


_DEFAULT_OPENAI_BASE_URL = "https://api.openai.com/v1"


class OpenAIProvider(LLMProvider):
    """Generic OpenAI-compatible chat completions provider.

    Two timeouts, intentionally distinct:

      - `timeout` (urllib socket timeout) — fires only when an
        individual socket read/write hangs for this many seconds.
        Slow-but-progressing streams never trigger it.
      - `total_timeout` (wall-clock per call) — hard ceiling on the
        whole `generate()` call. Necessary because the V1.5 sweep
        observed OpenRouter's GLM-4.5-Air free backend producing
        valid responses that took 2,177 seconds to stream — under
        the socket timeout, so urllib never aborted, but enough to
        single-handedly stall a 120-run sweep. Without `total_timeout`
        an unhealthy backend can hold the whole pipeline open
        indefinitely.

    `total_timeout` is enforced via a worker thread + Future. The
    underlying urllib request is *not* actively cancelled (urllib has
    no clean abort) — it leaks until the OS tears the socket down.
    For sweep callers this is acceptable: the abandoned thread costs
    a TCP connection slot, not loop-blocking time.
    """

    def __init__(
        self,
        model_name: str = "gpt-4o-mini",
        *,
        api_key: Optional[str] = None,
        base_url: Optional[str] = None,
        api_key_env: str = "OPENAI_API_KEY",
        timeout: int = 120,
        total_timeout: int = 90,
    ) -> None:
        key = api_key or os.environ.get(api_key_env)
        if not key:
            raise ValueError(
                f"{api_key_env} is not set; pass api_key= explicitly or "
                f"export the environment variable."
            )
        self.model_name = model_name
        self.api_key = key
        self.base_url = (base_url or _DEFAULT_OPENAI_BASE_URL).rstrip("/")
        self.timeout = timeout
        self.total_timeout = total_timeout
        self.last_used_tools: tuple = ()

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        self._reject_mutating_tools(tools or ())
        self.last_used_tools = tuple(tools) if tools else ()

        url = f"{self.base_url}/chat/completions"
        # Run the actual HTTP work in a worker thread so the caller
        # gets a hard wall-clock timeout regardless of how the upstream
        # decides to stream. See class docstring for the GLM-4.5-Air
        # incident that motivated this.
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as ex:
            fut = ex.submit(self._do_request, url, prompt)
            try:
                return fut.result(timeout=self.total_timeout)
            except concurrent.futures.TimeoutError as e:
                raise RuntimeError(
                    f"OpenAI-compatible API request to {url} exceeded "
                    f"total_timeout={self.total_timeout}s "
                    f"(model={self.model_name})"
                ) from e

    def _do_request(self, url: str, prompt: str) -> str:
        payload = {
            "model": self.model_name,
            "messages": [{"role": "user", "content": prompt}],
        }
        body = json.dumps(payload).encode("utf-8")
        # User-Agent matters: Groq sits behind Cloudflare and the
        # default `Python-urllib/3.x` UA gets bounced by WAF rule 1010
        # before the request reaches Groq's API. Identifying as Aegis
        # plus a realistic stdlib UA both gets through and makes our
        # traffic legible in their dashboards.
        request = urllib.request.Request(
            url,
            data=body,
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
                "User-Agent": "aegis-control-plane/1.0",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as resp:
                response_bytes = resp.read()
        except urllib.error.HTTPError as e:
            err_body = e.read().decode("utf-8", errors="replace")
            raise RuntimeError(
                f"OpenAI-compatible API returned HTTP {e.code} from {url}: "
                f"{err_body[:300]}"
            ) from e
        except urllib.error.URLError as e:
            raise RuntimeError(
                f"OpenAI-compatible API request to {url} failed: {e}"
            ) from e

        try:
            data = json.loads(response_bytes)
            return data["choices"][0]["message"]["content"] or ""
        except (json.JSONDecodeError, KeyError, IndexError, TypeError) as e:
            raise RuntimeError(
                f"Unexpected response shape from {url}: "
                f"{response_bytes[:300]!r}"
            ) from e

    @staticmethod
    def _reject_mutating_tools(tools) -> None:
        """Defence-in-depth: even though this provider currently doesn't
        forward tools, refuse on construction so a future caller adding
        tool dispatch can't accidentally let mutating callables through."""
        for tool in tools:
            name = getattr(tool, "__name__", "")
            if name in MUTATING_TOOL_NAMES:
                raise ValueError(
                    f"Tool '{name}' is a state-mutating callable and cannot "
                    f"be exposed to the LLM. Route writes through "
                    f"aegis.runtime.executor.Executor instead."
                )
