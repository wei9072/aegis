# Instructions for AI coding agents (Claude Code, Cursor, Aider, etc.)

This file tells you, the AI agent, how to help a user adopt Aegis V2
in **their** project. The user almost certainly typed something like
"add Aegis to my repo". Follow these commands verbatim; don't
paraphrase, don't skip.

If anything in this file conflicts with your interpretation of
README.md, follow this file. README is for human evaluators; this is
for you.

---

## What Aegis V2 is, in one paragraph

A judgment-free MCP server for LLM-driven coding workflows. The
single tool, `validate_file`, returns structured **findings** about
a proposed file write — syntax errors, structural-smell deltas,
security pattern matches, cross-file cycles, broken-caller lists.
Aegis does not block writes, does not rank findings, does not say
"this is bad". You — the consuming agent — decide which findings
to act on. Aegis only ensures the data is on the table.

Full design context: [`README.md`](README.md). Read that when the
user asks "what does this thing do".

---

## Setup — the canonical install sequence

V2 ships a single binary: `aegis-mcp`. Run these from the user's
home or workspace dir.

```bash
# 0. Prerequisites
git --version
cargo --version || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source "$HOME/.cargo/env"

# 1. Clone, enter
git clone https://github.com/wei9072/aegis ~/aegis
cd ~/aegis

# 2. Build + install. ~1-2 min first time.
cargo install --path crates/aegis-mcp

# 3. Verify (do not skip)
which aegis-mcp           # confirms $PATH installation
aegis-mcp --version 2>&1 | head -1 || echo "(aegis-mcp does not have --version; absence of 'command not found' is the success signal)"
```

After step 3 succeeds, `aegis-mcp` is ready to be configured as an
MCP server in your client.

Common breakage:

- `cargo: command not found` → restart shell or `source "$HOME/.cargo/env"`.
- `failed to compile aegis-core` → `rustup update` (need 1.74+).

---

## Integration — configure the MCP client

Aegis V2 is an MCP server. The user's existing agent (Cursor / Claude
Code / your own) already speaks MCP; you just need to register
`aegis-mcp` in their client config.

### Claude Code

Add to `~/.config/claude-code/mcp.json` (path may vary):

```jsonc
{
  "mcpServers": {
    "aegis": {
      "command": "aegis-mcp"
    }
  }
}
```

### Cursor

Add to Cursor's MCP settings (Settings → MCP):

```jsonc
{
  "aegis": {
    "command": "aegis-mcp"
  }
}
```

### Generic MCP client

Stdio transport, JSON-RPC 2.0, protocol version `2025-06-18`. No
flags. No env vars required.

After the client is configured, restart it. The tool `validate_file`
should appear in the available tools list.

---

## Using the `validate_file` tool

Schema (call this from the agent loop whenever it's about to write
a file):

```jsonc
{
  "name": "validate_file",
  "arguments": {
    "path": "src/auth.py",
    "new_content": "...",                  // required
    "old_content": "...",                  // optional — enables value_before/after/delta
    "workspace_root": "/abs/path/to/project"  // optional — adds Workspace-kind findings
  }
}
```

Return shape:

```json
{
  "schema_version": "v2.0",
  "findings": [
    { "kind": "...", "rule_id": "...", "file": "...", "range": {...}, "context": {...}, "user_acknowledged": false }
  ]
}
```

`kind` is one of: `syntax`, `signal`, `security`, `workspace`. There
is **no** `decision`, **no** `severity`, **no** rank ordering. You
decide what to do based on the findings + your own task context.

### When to pass `workspace_root`

- **Always pass it** when the change touches public API, shared
  modules, or anywhere a cycle could form. Adds cycle detection +
  broken-caller lists.
- **Skip it** for purely local edits where speed matters and cross-
  file impact is impossible (e.g., editing a test fixture or a
  README).

First call with a new `workspace_root` builds the workspace index
(parses every supported file once — typically 1-3 seconds for a
mid-sized repo). Subsequent calls only re-parse files whose mtime
changed; usually < 50ms.

### `aegis-allow` comments

If the user has `# aegis-allow: SEC003` (or `// aegis-allow: SEC003`,
or `aegis-allow: all`) on a line, Aegis still emits the matching
finding but sets `user_acknowledged: true`. **You should respect
this acknowledgement** — the user has explicitly opted out of that
rule for that line.

---

## Things you should NOT do

- **Do not block writes solely on a finding's existence.** A `signal`
  finding with `delta: 1` is not automatically bad. A `security`
  finding with `severity_hint: "block"` is a hint, not a verdict.
  Reason about it.
- **Do not retry until findings disappear.** If the user wrote
  `# TODO: revisit` deliberately, an `unfinished_marker_count`
  finding will fire forever. Acknowledge it (via `aegis-allow` or
  by reasoning) and move on.
- **Do not call `validate_file` on every keystroke.** Call before
  committing a file write — once per write attempt is the right
  cadence.
- **Do not look for V1 tools.** `validate_change`,
  `validate_change_with_workspace`, `attest_path` no longer exist.
  Everything is `validate_file` now.
- **Do not look for the `aegis` CLI binary.** V2 has only
  `aegis-mcp`. There is no `aegis check` / `aegis pipeline run` /
  `aegis attest` anymore.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
| :--- | :--- | :--- |
| MCP client doesn't see `validate_file` tool | Server didn't start | Run `aegis-mcp` directly in a shell — should hang waiting for stdio input. If it errors, the binary is broken; rebuild. |
| First `validate_file` call with `workspace_root` is slow | Workspace bootstrap | Expected on first call. Subsequent calls are fast. |
| Findings empty for a `.py` file | File has unsupported extension OR is in a path the registry doesn't recognize | Verify the path argument has a supported extension. See README's "Supported source languages" table. |
| `findings: []` even on broken syntax | Path uses an unsupported extension | Aegis emits no findings for unsupported file types — same as V1's `SKIP` decision. Switch to a supported language file or accept that Aegis has no opinion. |
| Inconsistent findings across calls | External writer modified files between calls | Aegis re-stats files via mtime; if mtime didn't change after a content change, the cache stays stale. Touch the file or restart `aegis-mcp`. |

---

## V1 → V2 migration cheat-sheet

| V1 | V2 |
| :--- | :--- |
| `aegis check path/to/file.py` | Call `validate_file` MCP tool with that file's content |
| `aegis-mcp` `validate_change` tool | Renamed to `validate_file` (same first three args) |
| `aegis-mcp` `validate_change_with_workspace` | Folded into `validate_file` — pass `workspace_root` |
| `aegis-mcp` `attest_path` | Removed. The agent already has the content in hand; pass it to `validate_file` instead. |
| `aegis pipeline run` | Removed. The agent IS the pipeline now. |
| `aegis scan` | Removed. Workspace bootstrap is implicit on first `workspace_root` call. |
| `decision: "BLOCK" / "WARN" / "PASS" / "SKIP"` | Removed. Read `findings[]` and reason. |
| `reasons[]` with `layer + decision + reason` | Replaced by `findings[]` with `kind + rule_id + context`. |
| `severity` on signals | Removed entirely. |
| `signals_after`, `signals_before`, `regression_detail`, `signal_deltas` | Folded into Signal-kind findings' `context.{value_before,value_after,delta}`. |
| Pre-commit hook with `aegis check` | Either run `cargo build` + your normal linter, or call `aegis-mcp` from a hook script using its stdio JSON-RPC. |

---

## When in doubt

Default behaviour: ignore findings whose `user_acknowledged: true`.
For everything else, reason about each finding's `context` against
what the user asked you to do, and decide. Don't fail-stop on
finding count; fail-stop only when the finding's facts genuinely
contradict the user's intent.
