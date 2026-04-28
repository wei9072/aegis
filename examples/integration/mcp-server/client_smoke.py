#!/usr/bin/env python3
"""Smoke-test a client-side `aegis-mcp` integration over stdio JSON-RPC.

This is intentionally a client control-flow example, not an agent retry
loop. A BLOCK verdict is surfaced as a stop signal and is not converted
into prompt feedback.
"""

from __future__ import annotations

import json
import os
import select
import shlex
import shutil
import subprocess
import sys
from itertools import count
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_COMMAND = "cargo run --quiet --package aegis-mcp"


class McpClient:
    def __init__(self, command: str) -> None:
        self._ids = count(1)
        self._proc = subprocess.Popen(
            shlex.split(command),
            cwd=REPO_ROOT,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )

    def close(self) -> None:
        if self._proc.poll() is None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._proc.kill()
                self._proc.wait(timeout=5)

    def request(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        timeout_seconds: int = 60,
    ) -> dict[str, Any]:
        if self._proc.stdin is None or self._proc.stdout is None:
            raise RuntimeError("MCP process pipes are not available")

        request_id = next(self._ids)
        payload: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
        }
        if params is not None:
            payload["params"] = params

        self._proc.stdin.write(json.dumps(payload) + "\n")
        self._proc.stdin.flush()

        ready, _, _ = select.select([self._proc.stdout], [], [], timeout_seconds)
        if not ready:
            raise TimeoutError(f"timed out waiting for MCP response to {method}")

        line = self._proc.stdout.readline()
        if not line:
            raise RuntimeError(
                f"MCP process exited before responding to {method}: "
                f"returncode={self._proc.poll()}"
            )

        response = json.loads(line)
        if response.get("id") != request_id:
            raise AssertionError(
                f"expected response id {request_id}, got {response.get('id')}"
            )
        if "error" in response and response["error"] is not None:
            raise RuntimeError(f"MCP error from {method}: {response['error']}")
        return response["result"]


def validate_change(client: McpClient, path: str, new_content: str) -> dict[str, Any]:
    result = client.request(
        "tools/call",
        {
            "name": "validate_change",
            "arguments": {
                "path": path,
                "new_content": new_content,
            },
        },
        timeout_seconds=15,
    )
    return result["structuredContent"]


def require_decision(verdict: dict[str, Any], expected: str, label: str) -> None:
    actual = verdict.get("decision")
    if actual != expected:
        raise AssertionError(
            f"{label}: expected decision={expected}, got {actual}; "
            f"verdict={json.dumps(verdict, sort_keys=True)}"
        )


def main() -> int:
    command = os.environ.get("AEGIS_MCP_COMMAND")
    if command is None:
        if shutil.which("cargo") is None:
            raise RuntimeError(
                "cargo is required for the default smoke-test command. "
                "Install Rust or set AEGIS_MCP_COMMAND to an existing "
                "aegis-mcp binary, for example: "
                'AEGIS_MCP_COMMAND="target/debug/aegis-mcp"'
            )
        command = DEFAULT_COMMAND

    client = McpClient(command)
    try:
        client.request("initialize", {"protocolVersion": "2025-06-18"})
        tools = client.request("tools/list")
        tool_names = [tool["name"] for tool in tools.get("tools", [])]
        if "validate_change" not in tool_names:
            raise AssertionError(f"validate_change not listed: {tool_names}")

        pass_verdict = validate_change(
            client,
            "example.py",
            "def add(left, right):\n    return left + right\n",
        )
        require_decision(pass_verdict, "PASS", "PASS case")
        print("PASS case: decision=PASS -> client_action=proceed")

        block_verdict = validate_change(
            client,
            "broken.py",
            "def broken(:\n    return 1\n",
        )
        require_decision(block_verdict, "BLOCK", "BLOCK case")
        print("BLOCK case: decision=BLOCK -> client_action=halt_and_surface")

        print("MCP client smoke test passed.")
        return 0
    finally:
        client.close()


if __name__ == "__main__":
    sys.exit(main())
