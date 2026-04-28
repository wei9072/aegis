# Aegis

[English](README.md) · [繁體中文](README.zh-TW.md)

> Aegis ensures bad decisions don't stick.

A system for preventing silent regressions in LLM-driven workflows.

> **If you are an AI coding agent** (Claude Code, Cursor, Aider, etc.)
> helping a user install or integrate Aegis, read
> [`AGENTS.md`](AGENTS.md) instead — it has imperative install
> commands, integration templates you can paste, and the framing
> rules you must respect. The rest of this README is for the human
> evaluating whether to adopt Aegis.

---

## What it is

Aegis is a constraint-based behavior harness for LLM systems.
(In the broad sense: a verification environment, not an agent driver.)

For the domain-independent framework definition, see
[`docs/framework.md`](docs/framework.md). `aegis-agent` is one
implementation case: Aegis applied to agent-driven code changes.

**Aegis does not write code, and does not tell the LLM how to
write code. It only judges whether the code an LLM produces is
allowed to stay.** The LLM (or whichever code generator you wrap)
keeps full agency over what to write; Aegis exercises agency only
over what passes its gates.

It enforces a local closed loop:
each proposed state transition is validated,
and regressions are rejected.

Instead of optimizing model behavior,
Aegis enforces explicit, verifiable constraints,
ensuring that invalid or regressive states do not persist.

---

## Why it exists

LLM systems fail in three ways that current tooling does not catch:

1. Multi-turn refactors accumulate regressions silently
2. LLM-described actions diverge from actual tool calls
3. Structural rules erode without anyone noticing

Aegis exists to make these failures visible and rejectable.

---

## Core mechanism

Aegis controls state transitions:

```
Sₙ → Sₙ₊₁ is allowed only if all constraints are satisfied
and no regression is detected.

Otherwise, the system rolls back to Sₙ.
```

Cost-aware rollback is the only cross-iteration consistent criterion.
Other checks (validation, structural constraints) act as local
guards, not global direction signals.

### What actually gets checked (V1.10, six layers)

| Layer | Checks | Where it runs |
| :--- | :--- | :--- |
| **Ring 0** — syntax | tree-sitter parse; ERROR / MISSING node → BLOCK | `aegis check`, MCP, pipeline |
| **Ring 0.5** — structural signals | `fan_out` (unique imports), `max_chain_depth` (longest method chain) — numeric only | `aegis check`, MCP, pipeline |
| **Cost regression** | `sum(signals_after) > sum(signals_before)` → BLOCK / ROLLBACK | MCP (when `old_content` given), pipeline (every iter) |
| **PlanValidator** | path safety / scope / dangerous_path / virtual-FS simulation | `aegis pipeline run` only |
| **Executor + Snapshot** | atomic apply with backup-dir rollback | `aegis pipeline run` only |
| **Stalemate / Thrashing detector** | sequence-level; halts the loop with a named reason | `aegis pipeline run` only |

`aegis check` and the MCP server expose the first three layers
(single-file judgement); the multi-turn pipeline adds the last
three (cross-iteration loop control).

---

## Design principles

- Do not write code; only judge code that is written
- Do not tell the model what to write; only what cannot stay
- Only reject what is verifiably bad
- No automatic learning
- No objective optimization

The first two are the load-bearing ones. They are why Aegis can
wrap any code-generating agent (Cursor, Claude Code, Aider, your
own pipeline) without becoming a competing one — Aegis exercises
*judge agency*; the wrapped agent keeps *author agency*.

---

## Guarantees

- Regressions are detected and rolled back when constraints are defined
- Invalid states are blocked at the validation layer
- All decisions are recorded in a machine-readable trace

---

## Non-goals

Aegis is not:

- An AI agent
- An optimizer
- A self-improving system

These are deliberate design choices, not future work.
Aegis will not evolve into any of these.
See [`docs/post_launch_discipline.md`](docs/post_launch_discipline.md)
for the explicit deferral list.

---

## Scope

Aegis enforces correctness within a single execution loop,
but does not adapt behavior across executions.

The loop is local, not global.

---

## Quickstart

As of V1.10, Aegis is a single Rust workspace producing two
binaries — `aegis` (CLI) and `aegis-mcp` (MCP stdio server). Zero
Python at runtime.

### Install

```bash
# Prerequisites: git + a Rust toolchain.
# If you don't have Rust:
#   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
#   source "$HOME/.cargo/env"

git clone https://github.com/wei9072/aegis && cd aegis
cargo build --release --workspace
```

Install system-wide so `aegis` / `aegis-mcp` end up on `$PATH`:

```bash
cargo install --path crates/aegis-cli
cargo install --path crates/aegis-mcp     # optional — MCP server
```

Cross-platform release artifacts (Linux x86_64/aarch64, macOS
x86_64/aarch64, Windows x86_64) ship via GitHub Releases — see V2.0
in [`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md).

### Static analysis (no LLM, no API key)

`aegis check` runs Ring 0 (syntax) + Ring 0.5 (structural signals
— fan-out, max chain depth) on any supported source file:

```bash
aegis languages                       # list supported languages
aegis check path/to/file.py           # human-readable signals
aegis check path/to/file.py --json    # machine-readable
```

The intent: drop this into a pre-commit hook or CI gate so bad
diffs never make it past the boundary you care about. See
[`docs/integrations/`](docs/integrations/) for paste-ready examples.

### LLM-backed multi-turn pipeline

`aegis pipeline run` drives the Planner → Validator → Executor
loop against your workspace. Provider config comes from environment
variables — supports any OpenAI-compatible endpoint (OpenAI,
OpenRouter, Groq):

```bash
export AEGIS_PROVIDER=openai          # or: openrouter | groq
export AEGIS_MODEL=gpt-4o-mini        # any model the provider exposes
export OPENAI_API_KEY=sk-...          # or AEGIS_API_KEY (provider-agnostic)

aegis pipeline run \
  --task "rename the foo helper to bar everywhere" \
  --root . \
  --max-iters 3
```

Every iteration prints a one-line summary (`iter 0 [abc12345] plan=continuing
patches=2 applied=true rolled_back=false`); add `--json` for a
machine-readable summary at the end. The loop stops when the
planner declares done, signals stalemate, signals thrashing, or
hits `--max-iters`. Cost-aware regression rollback fires
automatically if structural signals get worse mid-loop.

### MCP server (Cursor / Claude Code / your own agent)

```bash
aegis-mcp     # stdio JSON-RPC, MCP protocol 2025-06-18
```

Configure your MCP client per
[`docs/integrations/mcp_design.md`](docs/integrations/mcp_design.md);
the agent then calls `validate_change(path, new_content,
old_content?)` mid-loop and gets a `{decision, reasons,
signals_after, …}` verdict back. Pure observation — never coaches
the agent (see [Design principles](#design-principles)).

### `aegis chat` — interactive coding agent (V3)

Built on the same primitives, `aegis chat` is a substrate-mode
agent: pick any of three providers via env vars, drop into a REPL
with line editing + slash commands + markdown rendering. Three
modes auto-detected:

```bash
aegis chat "explain this concept"        # one-shot
echo "task" | aegis chat                 # pipe → one-shot
aegis chat                               # tty → interactive REPL
```

Provider env vars (first match wins):

```bash
# OpenAI-compat (covers OpenRouter / Groq / Ollama / vLLM / etc.)
export AEGIS_OPENAI_BASE_URL=https://openrouter.ai/api/v1
export AEGIS_OPENAI_API_KEY=sk-or-v1-...
export AEGIS_OPENAI_MODEL=meta-llama/llama-3.3-70b-instruct

# Anthropic
export AEGIS_ANTHROPIC_API_KEY=sk-ant-...
export AEGIS_ANTHROPIC_MODEL=claude-haiku-4-5

# Gemini
export AEGIS_GEMINI_API_KEY=AIza...
export AEGIS_GEMINI_MODEL=gemini-2.5-flash
```

Common flags:

```bash
aegis chat --tools --workspace .                # add Read/Glob/Grep tools
aegis chat --tools --mcp aegis-mcp              # mount aegis-mcp as a tool
aegis chat --verify                             # auto-detect test runner
aegis chat --cost-budget 5.0                    # terminate on cumulative regression
aegis chat --permission-mode read-only          # safest sandbox
```

The four V3 differentiation points (PreToolUse aegis-predict,
cross-turn cost tracking, verifier-driven done, stalemate detection)
are wired in; full usage walkthrough in
[`docs/v3_dogfood.md`](docs/v3_dogfood.md), design rationale in
[`docs/v3_agent_design.md`](docs/v3_agent_design.md).

---

## Integrations

You're already using Cursor / Claude Code / Aider / Copilot / your
own agent. Aegis is meant to be a **side-channel enforcement layer**
that doesn't ask you to switch tools.

| Boundary | Path | Status |
| :--- | :--- | :--- |
| Commit | [Git pre-commit hook](docs/integrations/git_pre_commit.md) | ✓ ready (5-line bash) |
| PR / merge | [GitHub Action / CI gate](docs/integrations/github_action.md) | ✓ ready (10-line YAML) |
| Agent decision | [MCP server](docs/integrations/mcp_design.md) | ✅ `validate_change` ready (`cargo install --path crates/aegis-mcp && aegis-mcp`) |

Pick whichever boundary fits your workflow; you can stack them.
Index + per-path detail: [`docs/integrations/`](docs/integrations/).

---

## Status

| Layer | State | Notes |
| :--- | :--- | :--- |
| Execution Engine | ✅ | Pipeline + Executor + cost-aware regression rollback. Native Rust loop in `aegis-runtime::native_pipeline`. |
| Static analysis | ✅ | Ring 0 (syntax) + Ring 0.5 (`fan_out`, `max_chain_depth`) shared by `aegis check` + `aegis pipeline run` + `aegis-mcp validate_change`. |
| Decision Trace | ✅ | `DecisionTrace` + 10-value `DecisionPattern` + 5-value `TaskVerdict`; Python-era cross-model evidence in [`docs/v1_validation.md`](docs/v1_validation.md). Rust re-validation is gated on LLM API budget (V1.8). |
| MCP server | ✅ | `aegis-mcp` — hand-rolled JSON-RPC 2.0 over stdio; one tool: `validate_change` per [`docs/integrations/mcp_design.md`](docs/integrations/mcp_design.md). |
| Cross-model sweep harness | 🟡 | `aegis pipeline run` works scenario-by-scenario; batch sweep (`aegis sweep`) is V1.8 backlog — gated on API budget. |
| Feedback Layer | ❌ | Out of scope by design — see [Non-goals](#non-goals) and [Critical Principle](docs/gap3_control_plane.md#critical-principle). Structurally enforced by `crates/aegis-decision/tests/contract.rs`. |

### Supported source languages (Ring 0 + Ring 0.5 signals)

Tier 2 multi-language support landed in V1.4–V1.7 of the Rust port
(see [`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md)). With
V1.10's Python deletion, every language listed below now gets both
**enforcement** (Ring 0 + Ring 0.5 via `aegis check`) and the
**refactor** half (`aegis pipeline run` against an OpenAI-compatible
LLM provider). Run `aegis languages` for the live registry.

| Language | Ring 0 syntax | Ring 0.5 fan-out | Ring 0.5 chain-depth | Extensions |
| :--- | :---: | :---: | :---: | :--- |
| Python | ✅ | ✅ | ✅ | `.py`, `.pyi` |
| TypeScript | ✅ | ✅ | ✅ | `.ts`, `.tsx`, `.mts`, `.cts` |
| JavaScript | ✅ | ✅ | ✅ | `.js`, `.mjs`, `.cjs`, `.jsx` |
| Go | ✅ | ✅ | ✅ | `.go` |
| Java | ✅ | ✅ | 🟡 | `.java` |
| C# | ✅ | ✅ | ✅ | `.cs` |
| PHP | ✅ | ✅ | ✅ | `.php`, `.phtml`, `.php5`, `.php7`, `.phps` |
| Swift | ✅ | ✅ | ✅ | `.swift` |
| Kotlin | ✅ | ✅ | ✅ | `.kt`, `.kts` |
| Dart | ✅ | ✅ | 🟡 | `.dart` |

🟡 = the default chain-depth walker under-counts on this language's
AST shape; per-language overrides are the planned fix path
(`LanguageAdapter::max_chain_depth`).

Adding a language is one Cargo dep + one adapter file under
`crates/aegis-core/src/ast/languages/` + one `.scm` query —
checklist in [`docs/multi_language_plan.md#per-language-work-checklist`](docs/multi_language_plan.md#per-language-work-checklist).

---

## Philosophy

> If Aegis starts learning automatically,
> it has violated its own design.

---

## License

MIT — see [`LICENSE`](LICENSE).

V1.10 — Rust workspace, zero Python at runtime. Cross-platform
release artifacts (Homebrew, npm, GitHub Releases) are templated
under [`packaging/`](packaging/); activation is the V2.0 milestone
in [`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md).
