# MCP server design + status

The git-hook and GitHub-Action integrations protect at the **commit
boundary**. They are post-hoc — by the time the hook fires, the
LLM has already produced and saved its work.

**Status (V1.10):** `validate_change` is shipped in the
`aegis-mcp` Rust binary — hand-rolled JSON-RPC 2.0 over stdio,
MCP protocol `2025-06-18`, ~250 LOC, zero Python at runtime.
`validate_diff` and `get_signals` from the design are deferred
until external demand justifies them.

Install with `cargo install --path crates/aegis-mcp`, run with
`aegis-mcp`. See [Configuration shape](#configuration-shape) below
for how to plug into Cursor / Claude Code.

The MCP server protects at the **decision boundary** — the agent
calls Aegis *before* writing files, gets a verdict, and decides
what to do with it. This is the integration that fits Cursor,
Claude Code, and any other MCP-aware client where the agent's
loop is the unit of operation.

This document pins the contract so future tools (`validate_diff`,
`get_signals`) can be built consistently if real demand justifies
them, per
[`docs/post_launch_discipline.md`](../post_launch_discipline.md).

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

## Configuration shape

How a Claude Code / Cursor user wires up `aegis-mcp`:

**Cursor** (`~/.cursor/mcp.json`):
```json
{
  "mcpServers": {
    "aegis": {
      "command": "aegis-mcp"
    }
  }
}
```

**Claude Code** (`.mcp.json` at project root, or `~/.claude.json`
for global):
```json
{
  "mcpServers": {
    "aegis": {
      "command": "aegis-mcp"
    }
  }
}
```

`aegis-mcp` reads JSON-RPC over stdio; no flags or env vars are
required. After config, restart the MCP client and `validate_change`
becomes available to the agent automatically. Agents that support
MCP discover and call tools opportunistically.

### Optional: PreToolUse hook (Claude Code) for hard enforcement

Soft prompting via CLAUDE.md asks the agent to call
`mcp__aegis__validate_change` before edits — but the agent can
ignore it. To **hard-enforce** that every Edit / Write / MultiEdit
goes through `aegis check` first, drop a PreToolUse hook into
`.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{"type": "command", "command": ".claude/hooks/aegis-precheck.sh"}]
      }
    ]
  }
}
```

The hook synthesizes the post-edit content, runs `aegis check`
on it, and exits non-zero (BLOCK) on Ring 0 / cost regression.
A reference implementation is in
[`templates/`](../../templates/).

---

## Implementation

The shipped server is in
[`crates/aegis-mcp/`](../../crates/aegis-mcp/) — hand-rolled
JSON-RPC 2.0 over stdio (~250 LOC, no `rmcp` dep). It links
directly against `aegis-core` for Ring 0 + Ring 0.5 signal
extraction.

Entry-point logic for `validate_change` (paraphrased):

```rust
// 1. Write new_content to a temp file with the right extension
// 2. Run aegis-core::ast::analyze_file_native on it (Ring 0 syntax + Ring 0.5 signals)
// 3. If old_content supplied:
//      - same on a temp file for old_content
//      - cost_after = sum(signals_after.values), cost_before = sum(signals_before.values)
//      - regressed = cost_after > cost_before
// 4. Map to {decision, reasons, signals_after, regression_detail}
// 5. Return as JSON-RPC tool result
```

No new decision logic — the MCP server is a thin adapter over the
same Ring 0 + Ring 0.5 + cost regression that `aegis check` and
`aegis pipeline run` use.

Install + run:

```bash
cargo install --path crates/aegis-mcp
aegis-mcp        # starts MCP server on stdio
```

Then configure your MCP client per [Configuration shape](#configuration-shape)
above.

---

## Status

✅ **`validate_change` shipped in `aegis-mcp` (V1.10).**
🟡 `validate_diff` + `get_signals` deferred until external demand
   justifies them (per
   [`docs/post_launch_discipline.md`](../post_launch_discipline.md)
   — wait for an issue saying "I need them for X").

The "reasons must be machine-parseable, not coaching" rule is
structurally enforced — `validate_change`'s response shape carries
no natural-language explanation field, and the
`crates/aegis-decision/tests/contract.rs` test pins that
`TaskVerdict` cannot grow `retry/feedback/hint/advice/guidance`
fields.

---

## Related

- [`docs/integrations/README.md`](README.md) — overview of all integration paths
- [`docs/gap3_control_plane.md`](../gap3_control_plane.md) — control-boundary design (the same rules apply across MCP, pre-commit, CI)
- [`docs/post_launch_discipline.md`](../post_launch_discipline.md) — when to build new integrations vs wait for demand
