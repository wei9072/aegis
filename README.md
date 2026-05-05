# Aegis

[English](README.md) · [繁體中文](README.zh-TW.md)

> Aegis describes facts. The agent decides what to do.

A judgment-free observation layer for LLM-driven coding workflows.

> **If you are an AI coding agent** (Claude Code, Cursor, Aider, etc.)
> helping a user install Aegis, read [`AGENTS.md`](AGENTS.md) — it
> has the imperative install commands you should run.

---

## What it is (V2)

Aegis is an MCP server that emits **findings** about a proposed file
write. It does not block, warn, allow, or otherwise pass judgment.
Each finding is a fact — "this regex matched here", "this signal
went up by 1", "this file imports another file that imports it back"
— with enough structured context for the consuming agent (the LLM)
to decide what to do.

**Aegis does not write code, does not gate writes, and does not
score outcomes.** It only describes what changed.

The previous V1 architecture (Ring 0/0.5/0.7/R2 with BLOCK/WARN/PASS
verdicts, multi-turn pipeline, cost-aware regression rollback,
stalemate/thrashing detectors) is gone in V2. Judgment lives where
it belongs: in the consuming agent's reasoning step.

---

## Why it exists

LLM systems still fail in three ways the surrounding tooling does
not catch:

1. Multi-turn refactors accumulate regressions silently
2. LLM-described actions diverge from actual tool calls
3. Structural rules erode without anyone noticing

Aegis exists to make these failures **visible**. Whether they're
acceptable in this context is a question for the agent or the
human; Aegis only ensures the data is on the table.

---

## How it works

Two infrastructure layers feed a single MCP tool.

```
┌─────────────────────────────────────┐
│ MCP Tool: validate_file             │
│   (path, new_content,               │
│    old_content?, workspace_root?)   │
└──────────────┬──────────────────────┘
               │ findings[]
               ▼
┌─────────────────────────────────────┐
│ Findings Generators                 │
│   Syntax · Signal · Security        │
│   Workspace                         │
└──────────────┬──────────────────────┘
               │
       ┌───────┴────────┐
       ▼                ▼
┌─────────────┐  ┌─────────────────┐
│ Layer 1     │  │ Layer 2         │
│ parse(file) │  │ WorkspaceIndex  │
│  → Tree     │  │ (mtime-cached)  │
└─────────────┘  └─────────────────┘
```

**Layer 1 — parse**: One tree-sitter call per file, shared across
every finding generator. No more per-signal Parser::new(). Always
returns a tree, even on broken syntax.

**Layer 2 — WorkspaceIndex**: Reverse index over per-file imports
and public symbols, mtime-cached so repeated MCP calls only re-parse
what actually changed.

**Findings**: Four kinds — Syntax, Signal, Security, Workspace —
described below. Every finding carries `file`, optional `range` and
`snippet`, and a structured `context` map. None carries severity.

---

## Finding kinds

| `kind` | What it means | Example `rule_id`s |
| :--- | :--- | :--- |
| **Syntax** | Tree-sitter found ERROR / MISSING nodes. | `ring0_violation` |
| **Signal** | A structural counter (14 of them). When `old_content` is supplied, `context` carries `value_before` / `value_after` / `delta`. | `fan_out`, `max_chain_depth`, `cyclomatic_complexity`, `nesting_depth`, `empty_handler_count`, `unfinished_marker_count`, `unreachable_stmt_count`, `mutable_default_arg_count`, `shadowed_local_count`, `suspicious_literal_count`, `unresolved_local_import_count`, `member_access_count`, `type_leakage_count`, `cross_module_chain_count`, `import_usage_count`, `test_count_lost` |
| **Security** | A specific anti-pattern matched (10 rules). `context.severity_hint` is a hint, not a verdict. | `SEC001`–`SEC010` (eval/exec, hardcoded secret, TLS-off, shell injection, SQL concat, CORS wildcard+credentials, JWT unsafe, insecure deserialization, weak crypto, weak RNG) |
| **Workspace** | Cross-file finding. Only emitted when `workspace_root` is supplied. | `cycle_introduced`, `public_symbol_removed`, `file_role` |

`aegis-allow: <rule_id>` (or `aegis-allow: all`) on the same or
previous source line marks `user_acknowledged: true` on the
matching finding instead of dropping it. The agent sees the
acknowledgement and can choose to honour it.

---

## Quickstart

V2 ships a single binary: `aegis-mcp` (the MCP server).

### Install

```bash
# Prerequisites: git + a Rust toolchain (1.74+).
git clone https://github.com/wei9072/aegis && cd aegis
cargo install --path crates/aegis-mcp
```

### Configure your MCP client

Point your MCP-aware client (Claude Code / Cursor / your own agent)
at the `aegis-mcp` binary over stdio. The exact configuration syntax
varies by client; the server itself takes no flags.

### The one tool: `validate_file`

```jsonc
{
  "name": "validate_file",
  "arguments": {
    "path": "src/auth.py",
    "new_content": "...",                  // required
    "old_content": "...",                  // optional — enables deltas
    "workspace_root": "/path/to/project"   // optional — adds Workspace findings
  }
}
```

Returns:

```json
{
  "schema_version": "v2.0",
  "findings": [
    {
      "kind": "security",
      "rule_id": "SEC009",
      "file": "src/auth.py",
      "range": { "start_line": 47, "start_col": 4, "end_line": 47, "end_col": 52 },
      "context": { "severity_hint": "block", "message": "weak hash …" },
      "user_acknowledged": false
    },
    {
      "kind": "signal",
      "rule_id": "unfinished_marker_count",
      "file": "src/auth.py",
      "context": { "value_before": 0, "value_after": 1, "delta": 1 },
      "user_acknowledged": false
    },
    {
      "kind": "workspace",
      "rule_id": "cycle_introduced",
      "file": "src/auth.py",
      "context": { "cycle": ["src/auth.py", "src/user.py", "src/auth.py"] },
      "user_acknowledged": false
    }
  ]
}
```

The first call with a `workspace_root` builds the workspace index
(parses every supported file once); subsequent calls reuse the cache
and only re-parse files whose mtime changed. No separate "scan"
step.

---

## Supported source languages

Run-time dispatch by file extension. Adding a language is a Cargo
dep + an adapter file under `crates/aegis-core/src/ast/languages/` +
a `.scm` import query — no other changes needed.

| Language | Layer 1 parse | Notes |
| :--- | :---: | :--- |
| Python | ✅ | `.py`, `.pyi` |
| TypeScript | ✅ | `.ts`, `.tsx`, `.mts`, `.cts` |
| JavaScript | ✅ | `.js`, `.mjs`, `.cjs`, `.jsx` |
| Go | ✅ | `.go` |
| Java | ✅ | `.java` |
| C# | ✅ | `.cs` |
| PHP | ✅ | `.php`, `.phtml`, `.php5`, `.php7`, `.phps` |
| Swift | ✅ | `.swift` |
| Kotlin | ✅ | `.kt`, `.kts` |
| Dart | ✅ | `.dart` |
| Rust | ✅ | `.rs` |

---

## Design principles

- **Describe facts, do not pass judgment.** Findings have no
  severity field. The consuming agent decides which findings
  matter and how to react.
- **Parse once, share the tree.** Every finding generator consumes
  a `ParsedFile`. No per-signal `Parser::new()`. No temp-file
  round-trip.
- **Workspace bootstrap is implicit.** First call with a
  `workspace_root` builds the index; subsequent calls hit the
  mtime cache. No separate scan tool, no manual init step.
- **No automatic learning, no objective optimization.** Aegis does
  not track success/failure across calls, does not adapt rules,
  does not score outcomes. State only carries the workspace cache.
- **One MCP tool, narrow surface.** `validate_file` and that's it.
  No `retry`, no `hint`, no `explain`. Agent reasoning is the
  agent's job.

---

## Status

| Layer | State |
| :--- | :--- |
| Layer 1 (parse + 11 language adapters) | ✅ |
| Layer 2 (WorkspaceIndex + mtime cache) | ✅ |
| Findings: Syntax + Signal + Security + Workspace | ✅ |
| MCP server (`aegis-mcp`) | ✅ |
| V1 binaries (`aegis`, `aegis pipeline run`, `aegis check`, `aegis attest`, `aegis scan`) | ❌ removed in V2 |

---

## License

MIT — see [`LICENSE`](LICENSE).

V2 — MCP-only architecture. Pipeline / runtime / providers / IR /
decision crates removed; judgment lives in the consuming agent.
