# MCP server design + minimum viable build

The git-hook and GitHub-Action integrations protect at the **commit
boundary**. They are post-hoc — by the time the hook fires, the
LLM has already produced and saved its work.

**Status (V0.x):** `validate_change` is implemented and shipped in
the `aegis_mcp` package. `validate_diff` and `get_signals` from the
design are deferred — added when external demand justifies them.
Install with `pip install -e ".[mcp]"`, run with `aegis-mcp`. See
[Configuration shape](#configuration-shape-illustrative) below for
how to plug into Cursor / Claude Code.

The MCP server protects at the **decision boundary** — the agent
calls Aegis *before* writing files, gets a verdict, and decides
what to do with it. This is the integration that fits Cursor,
Claude Code, and any other MCP-aware client where the agent's
loop is the unit of operation.

This document pins the interface so an implementation (`aegis-mcp`)
can be built without re-deriving the design. The build itself is
deferred per
[`docs/post_launch_discipline.md`](../post_launch_discipline.md) —
build when external demand justifies the cost.

---

## What MCP gives Aegis

[Model Context Protocol](https://modelcontextprotocol.io/) is the
standard Anthropic / Cursor / etc. settled on for "agents call
external tools". An `aegis-mcp` server registers itself as a tool
provider; the agent's runtime makes the tools available to the LLM
inside its loop.

Concretely, when Cursor / Claude Code is configured with
`aegis-mcp`, the agent can mid-conversation call:

```
agent → mcp_aegis.validate_change(path="src/foo.py", new_content="...")
mcp_aegis → {"decision": "BLOCK", "reasons": ["Ring 0: SyntaxError line 3"]}
agent ← receives verdict, decides what to do
```

The agent is the controller; Aegis is the gate. This matches
[Gap 3's control-boundary rule](../gap3_control_plane.md#1-control-boundary):
pipeline emits verdicts, controller (here: the agent's prompt loop)
decides what to do.

---

## Tools to expose

Three tools cover the core integration. Names + signatures pinned:

### 1. `validate_change` — the primary gate

Inspect a proposed file write and return the gate vocabulary's
verdict.

```jsonc
{
  "name": "validate_change",
  "description": "Run Aegis Ring 0 + PolicyEngine on a proposed file write. Returns BLOCK/WARN/PASS plus the reasons. Does NOT apply the change.",
  "input_schema": {
    "type": "object",
    "properties": {
      "path":        {"type": "string", "description": "Path the agent intends to write."},
      "new_content": {"type": "string", "description": "Full file contents the agent intends to write."},
      "old_content": {"type": "string", "description": "Optional: current file contents on disk, for cost-aware regression check."}
    },
    "required": ["path", "new_content"]
  }
}
```

Returns:

```jsonc
{
  "decision": "PASS" | "WARN" | "BLOCK",
  "reasons": [
    {"layer": "ring0", "decision": "block", "reason": "syntax_invalid", "detail": "expected ':' at line 3"}
  ],
  "signals_before": {"fan_out": 5,  "max_chain_depth": 1},
  "signals_after":  {"fan_out": 22, "max_chain_depth": 4},
  "regression_detail": {"fan_out": 17.0, "max_chain_depth": 3.0}
}
```

`signals_before` / `signals_after` only populated if `old_content`
was supplied. `regression_detail` only populated when
`signals_after` cost > `signals_before` cost.

### 2. `validate_diff` — multi-file proposals

Same shape, but for an agent proposing changes across multiple
files at once (e.g., a small refactor):

```jsonc
{
  "name": "validate_diff",
  "description": "Run Aegis on a multi-file diff. Returns aggregate verdict + per-file breakdown.",
  "input_schema": {
    "type": "object",
    "properties": {
      "diff": {"type": "string", "description": "Unified diff format."}
    },
    "required": ["diff"]
  }
}
```

### 3. `get_signals` — read-only inspection

For agents that want to query structural metrics without proposing
a change. Useful when planning ("is fan_out already too high in
this file before I add to it?").

```jsonc
{
  "name": "get_signals",
  "description": "Extract Aegis Ring 0.5 structural signals from a file. Read-only.",
  "input_schema": {
    "type": "object",
    "properties": {
      "path": {"type": "string"}
    },
    "required": ["path"]
  }
}
```

Returns the raw signal map: `{"fan_out": 5, "max_chain_depth": 1, ...}`.

---

## What is NOT exposed (and why)

These tools are deliberately omitted. Each one would push Aegis
across the
[critical principle](../gap3_control_plane.md#critical-principle)
boundary from constraint system to optimizer.

| Tool that won't exist | Why not |
| :--- | :--- |
| `suggest_fix(verdict)` | The agent must figure out the fix. Aegis emits verdicts; it does not coach. |
| `apply_with_retry(...)` | Auto-retry would make the MCP server a goal-seeker. Retry is the agent's decision, never Aegis's. |
| `explain_block(verdict, llm_friendly=True)` | Explanations would be Aegis injecting hints into the agent's prompt. The reasons returned by `validate_change` are structured (`{layer, reason, detail}`), not natural-language coaching. |
| `predict_next_change(...)` | Predicting the agent's next move would require modeling the agent. Out of scope. |

If a future PR proposes any of these, the response is "this changes
Aegis from a decision system to a goal-seeker — see
docs/gap3_control_plane.md#critical-principle". Same conversation
as auto-retry inside the pipeline.

---

## Configuration shape (illustrative)

How a Claude Code / Cursor user would configure `aegis-mcp` once
it's built:

**Cursor** (`~/.cursor/mcp.json`):
```jsonc
{
  "mcpServers": {
    "aegis": {
      "command": "aegis-mcp",
      "args": ["serve"],
      "env": {
        "AEGIS_HOME": "/Users/me/code/aegis"
      }
    }
  }
}
```

**Claude Code** (`~/.claude/mcp_servers.json` or equivalent):
```jsonc
{
  "aegis": {
    "command": "aegis-mcp",
    "args": ["serve"]
  }
}
```

After config, the agent automatically has the three tools available.
No prompt-engineering needed; agents that support MCP discover and
call tools opportunistically.

---

## Implementation sketch

When the build happens, `aegis-mcp` is roughly 200 lines of Python:

```python
# aegis_mcp/server.py — illustrative, not the real implementation
from mcp import Server  # python-mcp-sdk
from aegis.enforcement.validator import Ring0Enforcer
from aegis.policy.engine import PolicyEngine
from aegis.analysis.signals import SignalLayer

server = Server("aegis")

@server.tool("validate_change")
def validate_change(path: str, new_content: str, old_content: str | None = None):
    # 1. Write new_content to a temp file
    # 2. Run Ring0Enforcer on it
    # 3. Run SignalLayer extract → PolicyEngine evaluate
    # 4. If old_content supplied: extract signals on both, compute regression
    # 5. Aggregate into the verdict shape above and return
    ...

# ... validate_diff, get_signals same shape

if __name__ == "__main__":
    server.run_stdio()
```

The aegis-internal types (`Ring0Enforcer`, `SignalLayer`,
`PolicyEngine`) all already exist. The MCP server is a thin
adapter — no new decision logic.

---

## Status

✅ **`validate_change` shipped in `aegis_mcp/server.py`.**
🟡 `validate_diff` + `get_signals` deferred until external demand
   justifies them (per
   [`docs/post_launch_discipline.md`](../post_launch_discipline.md)
   — wait for an issue saying "I need them for X").

The implementation is ~150 lines of Python wrapping existing aegis
types (`Ring0Enforcer`, `SignalLayer`, `PolicyEngine`); the contract
above is what got built. Tested at
[`tests/test_mcp_server.py`](../../tests/test_mcp_server.py),
including a structural test pinning the "reasons must be
machine-parseable, not coaching" rule.

Install + run:

```bash
pip install -e ".[mcp]"
aegis-mcp        # starts MCP server on stdio
```

Then configure your MCP client per [Configuration shape](#configuration-shape-illustrative)
above.

---

## Related

- [`docs/integrations/README.md`](README.md) — overview of all integration paths
- [`docs/gap3_control_plane.md`](../gap3_control_plane.md) — control-boundary design (the same rules apply across MCP, pre-commit, CI)
- [`docs/post_launch_discipline.md`](../post_launch_discipline.md) — when to build new integrations vs wait for demand
