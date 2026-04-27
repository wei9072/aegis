# V3 dogfood — running aegis-agent against a real codebase

Once V3 (V3.0–V3.8 + the chat REPL polish) shipped, the natural test
is to point `aegis chat --tools` at a real codebase and see what
breaks. This document captures the recipe + a worked example
against a small Python codebase (`fit-coche`).

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

## What's not yet wired

These are deliberate scope cuts in V3:

- **No Edit / Write tools** — agent can read but not mutate files.
  Add a write tool when there's a clear scope-and-permissions story
  (the V3 framing means writes need both the write tool AND the
  PreToolUse aegis-predict gate active).
- **No Bash tool** — same reasoning. `--permission-mode danger-full-access`
  exists for when this lands.
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
