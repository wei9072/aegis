# V3 dogfood — running aegis-agent against a real codebase

> **⚠ Recipe only — evidence section pending real-LLM runs.** This
> document captures the canonical setup + the three usage modes. The
> "what actually happened in dogfood" section is intentionally empty
> until the project owner has run `aegis chat` against a real
> codebase with a real provider. Once that happens, append a
> "## Findings" section with: scenarios attempted, where the agent
> succeeded, where it surfaced one of the four V3 differentiation
> point verdicts (predict-block / cost-budget / verifier-rejected /
> stalemate), and any framing-relevant surprises.
>
> **V3.9 (2026-04-28) — major capability + UX expansion** (see also
> `docs/multi_language_plan.md`). Phase 0–7 of the V3.9 plan landed:
>
> - Rust language support in ring 0 (aegis can scan its own crates)
> - `aegis setup` interactive wizard + `aegis chat --print-config-template`
> - Multi-line input (`\` continuation), `--resume` defaults to latest,
>   `/sessions` listing, `/aegis` status, `/init` workspace bootstrap,
>   `--verbose`, friendly provider error hints, visible PreToolUse
>   rejection banner (`[aegis] BLOCK Edit foo.rs: cost 12 → 17 …`)
> - Provider model registry + alias resolver (sonnet/opus/haiku/4o/etc.)
>   + preflight check (rejects requests > ctx_window — no silent
>   truncation, ever)
> - Anthropic ephemeral cache_control on system prompt (~90% input
>   cost cut on stable system prompts)
> - REPL `/model <alias>` mid-session model switch
> - Plan mode (writes route through predictor, disk untouched) — the
>   aegis-shaped counterpart to claw-code's prose-summary plan mode
> - Path-glob permission rules + PermissionPrompter trait
> - Session compaction with fact-shaped summary (no LLM round-trip,
>   guarded by the new `no_coaching_in_summary` contract test —
>   four contract tests now structurally defend the framing)
> - Bash V0 (subprocess + 60s deadline + `>`/`>>`/`tee` redirect
>   parser banner; deeper aegis-aware predict integration deferred
>   until heuristic surface area is justified by dogfood)
> - Four extra tools: TodoWrite, WebFetch, WebSearch (DDG HTML),
>   AskUserQuestion (with stdin / scriptable prompter)
>
> Workspace tests grew from ~330 (V3.8) to **433** (V3.9). Build is
> warning-free across all crates. None of the four contract tests
> fired — framing intact.

Once V3 (V3.0–V3.8 + the chat REPL polish) shipped, the natural test
is to point `aegis chat --tools` at a real codebase and see what
breaks. This document captures the recipe; the project owner will
add the actual evidence below as it accumulates.

## Setup

```bash
# Build everything once.
cd ~/harness/aegis
cargo install --path crates/aegis-cli --locked
cargo install --path crates/aegis-mcp --locked

# Pick a provider — any of the three works.
# OpenAI-compat (covers OpenRouter / Groq / Ollama / vLLM / etc.):
export AEGIS_OPENAI_BASE_URL=https://openrouter.ai/api/v1
export AEGIS_OPENAI_API_KEY=sk-or-v1-...
export AEGIS_OPENAI_MODEL=meta-llama/llama-3.3-70b-instruct

# Or Anthropic:
export AEGIS_ANTHROPIC_API_KEY=sk-ant-...
export AEGIS_ANTHROPIC_MODEL=claude-haiku-4-5

# Or Gemini:
export AEGIS_GEMINI_API_KEY=AIza...
export AEGIS_GEMINI_MODEL=gemini-2.5-flash
```

## Three usage modes

### Mode 1 — pure chat REPL (no tools, no MCP)

```bash
aegis chat
```

Drops into the REPL with markdown rendering + spinner + line-edit /
history / slash-command tab-complete. The LLM has no tools — pure
conversation. Useful for trying out a new model.

### Mode 2 — chat with read-only inspection tools

```bash
aegis chat --tools --workspace ~/fit-coche
```

LLM gets `Read` / `Glob` / `Grep` tool definitions. It can:
- `Glob {"pattern":"**/*.py"}` to list source files
- `Read {"path":"src/main.py"}` to inspect any file
- `Grep {"pattern":"def login"}` to find call sites

No write / edit / shell tools — `--tools` is strictly read-only. The
agent can analyse but not mutate. Workspace-relative paths only;
`../` traversal is rejected.

### Mode 3 — chat with MCP tool servers mounted

```bash
aegis chat --tools --mcp aegis-mcp --workspace ~/fit-coche
```

Mounts the local `aegis-mcp` binary as an MCP tool source. The LLM
sees `validate_change` advertised alongside the built-in `Read` /
`Glob` / `Grep`, dispatched via `MultiToolExecutor` (first-source-wins
on duplicate tool names — built-ins precede MCP).

Combine multiple `--mcp` flags to mount more servers (e.g.
filesystem MCP + aegis-mcp).

## What the four V3 differentiation points look like in chat

### 1. PreToolUse aegis-predict
Currently inactive in chat mode (no built-in Edit / Write tool).
Becomes load-bearing once a write tool is mounted (V3 follow-up).

### 2. Cross-turn cost tracking
Type `/cost` at the REPL prompt to see the `CostTracker` snapshot:

```
you> /cost
  src/foo.py  baseline=12.00  current=15.00  regression=3.00
  src/bar.py  baseline=8.00   current=8.00   regression=0.00
  cumulative regression = 3.00
```

Set `--cost-budget 5.0` to terminate the session if cumulative
regression exceeds the budget.

### 3. Verifier-driven done
Add `--verify` to auto-detect the project's test runner
(`Cargo.toml` → `cargo test`, `pyproject.toml` → `pytest`,
`package.json` → `npm test`, `go.mod` → `go test ./...`). For
polyglot repos all detected runners must pass (CompositeVerifier).

```bash
aegis chat --tools --workspace ~/fit-coche --verify
```

When the LLM signals "done" (no more tool_use), the verifier runs.
If it fails → `StoppedReason::PlanDoneVerifierRejected` with the
test stderr in the rationale. The runtime does NOT auto-retry — you
see the failure, decide whether to refine the task and start a new
turn.

### 4. Stalemate detection
If the LLM keeps inspecting the same files without movement (cost
total unchanged for 3 successive iterations within a turn), the
runtime terminates with `StoppedReason::StalemateDetected`. No retry,
no coaching string injected. Visible in REPL as a status footer.

## Slash commands

| Command | Effect |
| :--- | :--- |
| `/exit`, `/quit` | leave the session |
| `/help` | list commands |
| `/reset` | clear conversation history + cost tracker + stalemate detector |
| `/cost` | print `CostTracker` snapshot |
| `/history` | print message count |

Tab-completion works on slash commands.

## What's not yet wired (V3.9 deferral list)

These are deliberate scope cuts. Each one has a principled reason
for waiting (per `post_launch_discipline.md`); revisit when a real
consumer asks for them with a concrete use case.

- **A6 streaming spinner** — pure UX polish. Streaming text already
  appears live for OpenAI-compat; spinner during tool calls would
  be nice but isn't blocking real work.
- **B5.5 OpenAI prefix cache** — provider-specific extension; needs
  real dogfood evidence that someone is paying for it before adding
  the wire surface area.
- **B5.6 Cost split** — distinguishing `cached_input_tokens` vs
  `input_tokens` in the cost tracker. Deferred until B5.4/5.5 have
  shipped real cache hits in dogfood (otherwise we have nothing to
  measure).
- **B6 deep aegis-aware Bash predict** — the V0 banner exists. The
  full BLOCK semantic needs content synthesis (knowing what
  `cmd > path` would actually write), which is heuristic-fragile.
  Lands incrementally as dogfood reveals real cases worth catching.
- **/memory slash command** — aegis has no memory store of its
  own; the existing Claude Code memory sits one level up from the
  chat runtime. Wiring needs an aegis-side memory abstraction
  first.
- **Project-local `aegis.toml` loader** — `/init` writes the file,
  but `cmd_chat` doesn't read it yet. Lands when a real consumer
  edits one and wants it picked up automatically.
- **Streaming for Anthropic / Gemini** — only OpenAI-compat truly
  streams in V3.8. Anthropic and Gemini use the non-streaming default
  (full response then callback replay). UX degrades to "wait, then
  see full text" — still functional, just less live.

## How to file a dogfood-feedback issue

If you run `aegis chat` against your codebase and find:

- A tool the agent tried to call that we don't expose
  → file with the tool name + intended use case
- A verifier auto-detect miss (your project structure)
  → file with the project markers + the test command you'd run
- A friction in the REPL UX (slash command missing, render glitch)
  → file with the input + expected vs actual output
- A framing concern (the agent did something coaching-shaped)
  → highest priority — these are red-line violations

Issues at https://github.com/wei9072/aegis/issues with the
`v3-dogfood` label.

---

The real test of V3 is whether `aegis chat --tools --mcp aegis-mcp`
on a polyglot repo produces a usable agent that respects the four
differentiation points without manual babysitting. The mode
documented here is the floor; everything else is feedback-driven.
