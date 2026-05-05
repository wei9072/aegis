# Integrations

V2 has exactly one integration: **the MCP server**.

```bash
cargo install --path crates/aegis-mcp
```

Then point your MCP-aware client (Claude Code / Cursor / your own
agent) at the `aegis-mcp` binary over stdio. See
[`AGENTS.md`](../../AGENTS.md) for client-specific config snippets
and the `validate_file` tool contract.

---

## V1 integrations removed

V1 shipped three integration paths — Git pre-commit hook, GitHub
Action / CI gate, and MCP server. V2 keeps only the MCP server.

The pre-commit hook and GitHub Action were thin wrappers around
`aegis check`, which itself was a thin wrapper around the V1
`validate_change` decision logic. With V2's "describe facts, agent
decides" architecture, there is no `decision: BLOCK` to drive a
pass/fail exit code from a hook — and synthesizing one from
findings would re-introduce the judgment layer that V2 removed.

If you need a CI gate, run your real toolchain (`cargo build`,
`tsc`, `pyright`, `ruff`, your test suite) — those produce
deterministic pass/fail and they understand newer language syntax
than tree-sitter does. Let aegis-mcp focus on what it's good at:
giving the LLM agent rich context mid-loop.
