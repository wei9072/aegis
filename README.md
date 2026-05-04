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
[`docs/framework.md`](docs/framework.md). This repository is the
reference implementation on the code-change domain: Aegis applied
to file-write transitions proposed by any upstream agent
(Cursor / Claude Code / Aider / your own).

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

### What actually gets checked (V1.10, eight layers)

| Layer | Checks | Where it runs |
| :--- | :--- | :--- |
| **Ring 0** — syntax | tree-sitter parse; ERROR / MISSING node → BLOCK | `aegis check`, MCP, pipeline, `aegis attest` |
| **Ring 0.5** — structural signals | 14 single-pass AST counters across 3 severity tiers — **block** (`empty_handler_count`, `unreachable_stmt_count`, `mutable_default_arg_count`, `shadowed_local_count`, `suspicious_literal_count`, `unresolved_local_import_count`, `unfinished_marker_count`, `test_count_lost`), **warn** (`fan_out`, `max_chain_depth`, `cyclomatic_complexity`, `nesting_depth`, `cross_module_chain_count`, `import_usage_count`), **info** (`member_access_count`, `type_leakage_count`). Severity decides whether a regression blocks. See [`signal_layer_pyapi.rs::severity_for`](crates/aegis-core/src/signal_layer_pyapi.rs). | `aegis check`, MCP, pipeline |
| **Ring 0.7** — security | 10 boolean rules (`SEC001`–`SEC010`): shell injection, SQL injection, JWT verify-disabled, weak crypto (md5/sha1), insecure RNG in security context, dangerous deserialization (`pickle`/`yaml.load`/`marshal`/Java `readObject`), …. Per-line opt-out: `// aegis-allow: SEC00X` (or `# aegis-allow: …`). | `aegis check`, MCP, pipeline, `aegis attest` |
| **Cost regression** | Per-signal delta — **any block-severity signal grew → BLOCK; any warn-severity signal grew → WARN** (reason recorded, doesn't fail by itself). The earlier sum-based check was retired because cross-signal cancellation hid real regressions. | MCP (when `old_content` given), pipeline (every iter) |
| **Ring R2** — workspace structure | Cross-file checks single-file `validate_change` can't do: **cycle introduction** (would the change create a new module import cycle?), **public-symbol-removed-while-callers-remain**, file-role classification (entry / core / hub / ordinary), z-scores of fan_in / fan_out / signals against the project baseline. `entry`-role files auto-suppress the `fan_out` warn (high fan_out is the expected shape). | `aegis-mcp validate_change_with_workspace`, `aegis attest --workspace`, pipeline |
| **PlanValidator** | path safety / scope / dangerous_path / virtual-FS simulation | `aegis pipeline run` only |
| **Executor + Snapshot** | atomic apply with backup-dir rollback | `aegis pipeline run` only |
| **Stalemate / Thrashing detector** | sequence-level; halts the loop with a named reason | `aegis pipeline run` only |

`aegis check` and the MCP server expose the first five layers
(single-file + workspace judgement); the multi-turn pipeline adds
the last three (cross-iteration loop control).

Each verdict carries one of four `decision` values:

- **PASS** — every gate passed.
- **WARN** — at least one warn-severity gate fired (e.g. heuristic
  signal regressed); the change is allowed but the reason is
  surfaced.
- **BLOCK** — at least one block-severity gate fired (Ring 0,
  Ring 0.7, block-severity Ring 0.5 regression, Ring R2 cycle, …).
- **SKIP** — the file extension has no language adapter
  (`.md`, `.toml`, `.json`, …); aegis has no opinion. Returning
  BLOCK here would just confuse upstream agents editing markdown.

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

`aegis check` runs Ring 0 (syntax) + Ring 0.5 (the 14 structural
signals) + Ring 0.7 (10 security rules) on any supported source file:

```bash
aegis languages                       # list supported languages
aegis check path/to/file.py           # human-readable signals
aegis check path/to/file.py --json    # machine-readable
```

For an entire workspace — parallel scan with persistent mtime+size
cache (rescans on a maintained codebase finish in <1s) plus
import-graph cycle detection across files:

```bash
aegis scan --workspace .              # parallel scan + cycle detection
aegis scan --workspace . --top 20     # top-N highest-cost files
aegis scan --no-cache --no-cycles     # skip the cache / skip cycle pass
```

Post-write attestation — read the on-disk content and run every
**absolute** check (Ring 0 + Ring 0.7 + optional Ring R2 cycle).
Intended for `PostToolUse` hooks / CI so writes that bypass the
pre-write gate still get judged. Appends a sha256-stamped JSONL
row to `<workspace>/.aegis/attestations.jsonl`:

```bash
aegis attest path/to/file.py --workspace .
aegis attest path/to/file.py --workspace . --json    # machine-readable
```

Setup wizard — interactive TOML writer for
`~/.config/aegis/config.toml` (provider, base URL, model, API-key
env var). Doesn't export anything to your shell:

```bash
aegis setup
```

The intent: drop these into a pre-commit hook, CI gate, or hook
chain so bad diffs never make it past the boundary you care about.
See [`docs/integrations/`](docs/integrations/) for paste-ready examples.

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
| Static analysis | ✅ | Ring 0 (syntax) + Ring 0.5 (14 severity-tagged signals) + Ring 0.7 (10 security rules, `SEC001`–`SEC010`) + Ring R2 (cross-file cycle / public-symbol-removed / file-role + z-scores) shared by `aegis check` + `aegis scan` + `aegis attest` + `aegis pipeline run` + `aegis-mcp validate_change` / `validate_change_with_workspace`. |
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
| Rust | ✅ | ✅ | ✅ | `.rs` |

🟡 = explicitly verified to under-count on this language's AST shape;
per-language overrides are the planned fix path
(`LanguageAdapter::max_chain_depth`). **Note:** the default walker is
union-of-known-shapes across grammars — only Java/Dart have been
explicitly hand-verified, so other languages may also under-count
silently if their grammar's receiver field name differs from the
fallback list. File a bug with a minimal repro if you hit a case the
walker misses.

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
