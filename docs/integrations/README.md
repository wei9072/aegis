# Integrations — adding Aegis without changing your workflow

You're already using Cursor / Claude Code / Aider / Copilot / Continue
/ your-own-agent. Aegis should not require you to switch tools. The
right shape is a **side-channel enforcement layer** that sits at one
of three boundaries:

```
   ┌──────────────────────────┐
   │  Your AI coding tool     │  (unchanged)
   │  (Cursor/Aider/etc.)     │
   └────────────┬─────────────┘
                │ writes files
                ▼
   ┌──────────────────────────┐
   │  Your codebase           │
   └────────────┬─────────────┘
                │
        ┌───────┼────────┐
        │       │        │
        ▼       ▼        ▼
     [git]   [PR/CI]   [MCP]
       │        │        │
       └────────┴────────┘
                │
            Aegis checks
        (rejects bad transitions)
```

Three boundaries → three ready-to-use integrations:

| # | Path | Best for | Setup time | Status |
| :--- | :--- | :--- | :--- | :--- |
| 1 | [Git pre-commit hook](git_pre_commit.md) | Solo developers, side projects | 2 min | ✓ ready |
| 2 | [GitHub Action / CI gate](github_action.md) | Teams with PR review | 5 min | ✓ ready |
| 3 | [MCP server](mcp_design.md) | Cursor / Claude Code users | 5 min config | ✅ `validate_change` shipped (`pip install -e .[mcp]` + `aegis-mcp`) |

The 5-path discussion (including LSP plugin and per-tool plugins
like Aider) is in
[`docs/post_launch_discipline.md`](../post_launch_discipline.md) —
those paths are deferred until evidence justifies them.

---

## Which one fits you

- **You're a solo dev pushing to your own repo** → start with the
  pre-commit hook. Five lines of bash. Catches structural
  regressions before they enter git history.
- **You're on a team, every change goes through PR review** → the
  GitHub Action gives every PR an Aegis check status. Same effect
  as pre-commit but at the merge boundary, and visible to reviewers.
- **You're using Cursor or Claude Code with MCP** → install with
  `pip install -e ".[mcp]"`, run `aegis-mcp`, configure per
  [the MCP doc](mcp_design.md). Only `validate_change` exposed in
  V0.x; ask for `validate_diff` / `get_signals` if you need them.

You can stack them. The git hook + CI gate + MCP server are
complementary: each catches a different timing of the same kind of
mistake.

---

## What Aegis enforces (regardless of integration)

Whatever path you pick, Aegis's verdict vocabulary stays the same:

- **Ring 0** — syntax / circular dependency violations → `BLOCK`
- **Ring 0.5** — structural signals (fan_out, max_chain_depth, etc.)
  → `OBSERVE`
- **PolicyEngine** — rule table over signals (e.g.
  `fan_out > 20 → BLOCK`)
- **Cost-aware regression** — multi-iteration runs only:
  post-apply structural cost > pre-apply → `ROLLBACK`

For a single commit / PR, the relevant gates are Ring 0 + PolicyEngine.
For multi-turn agent flows (where the LLM iterates), the
regression detection becomes the load-bearing piece — that's the
MCP path.

---

## What Aegis does NOT do across any integration

The framing from the project's
[critical principle](../gap3_control_plane.md#critical-principle)
applies here too:

- ❌ **No automatic retry.** Aegis tells you a change is bad. The
  agent / human decides whether to retry. Aegis does not loop.
- ❌ **No prompt rewriting.** Aegis does not feed verdicts back into
  the LLM as "here's how to fix it". The verdict is pure observation.
- ❌ **No model-of-the-developer's-intent.** Aegis judges code state,
  not "what you meant".

These are the same rules whether the integration is a git hook, CI
gate, or MCP tool call. They keep Aegis a *constraint system*, not
an *optimizer*.
