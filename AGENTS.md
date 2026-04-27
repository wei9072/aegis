# Instructions for AI coding agents (Claude Code, Cursor, Aider, etc.)

This file tells you, the AI agent, how to help a user adopt Aegis
in **their** project. The user almost certainly typed something like
"add Aegis to my repo" — they will not read installation docs. You
will. Follow the commands here verbatim; don't paraphrase, don't skip.

If anything in this file conflicts with your interpretation of
README.md, follow this file. README is for human evaluators; this
file is for you.

---

## What Aegis is, in one paragraph

A behavior harness for LLM-driven workflows. It rejects code-state
transitions that worsen structural metrics, blocks syntax/cycle
violations at write time, and emits a machine-readable trace of
every gate decision. **It does not generate code. It does not
optimize behavior. It does not retry on failure.** You and the
user's existing tools handle generation; Aegis sits as a side-channel
enforcement layer.

Full design context: [`README.md`](README.md) and
[`docs/v1_validation.md`](docs/v1_validation.md). Read those when
the user asks "what does this thing actually do".

---

## Setup — the canonical install sequence

V1.10 — Aegis is now a single Rust workspace, **zero Python at
runtime**. Run these in order from the user's home or workspace dir.

```bash
# 0. Prerequisites — check before installing.
git --version              # any recent
cargo --version || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source "$HOME/.cargo/env"

# 1. Clone, enter.
git clone https://github.com/wei9072/aegis ~/aegis
cd ~/aegis

# 2. Build (release).  ~1-2 min first time; <10s incremental.
cargo build --release --workspace

# 3. VERIFY (do not skip).
./target/release/aegis languages          # prints supported languages
./target/release/aegis check Cargo.toml   # zero-signal smoke test
```

Install system-wide (so `aegis` is on `$PATH`):

```bash
cargo install --path crates/aegis-cli
cargo install --path crates/aegis-mcp     # optional — MCP server
```

After step 3 passes, the `aegis` CLI works without a Python
interpreter on the box. Common causes if it doesn't build:

- `cargo: command not found` → restart shell or `source "$HOME/.cargo/env"`.
- `failed to compile aegis-core` → ensure your Rust toolchain is recent (1.74+); `rustup update`.

Cross-platform release artifacts (Linux x86_64/aarch64, macOS
x86_64/aarch64, Windows x86_64) ship via GitHub Releases — see
V2.0 in [`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md).

---

## Integration — pick exactly one for the user's situation

Ask the user (or infer from their workflow) which boundary they want
Aegis to enforce at:

| If the user... | Use this path |
| :--- | :--- |
| Has a personal project, wants `git commit` to block bad changes | **A. Pre-commit hook** |
| Has a team repo with PRs, wants reviewers to see Aegis status | **B. GitHub Action** |
| Is in Cursor / Claude Code / has their own LLM agent loop | **C. Wrap their LLM with `LLMGateway`** |

Don't stack paths until the user has lived with one for at least a
day. They are complementary, but layering before the basics work is
how you build untrustable setups.

### Path A — Pre-commit hook

Drop this verbatim into the user's project at `.git/hooks/pre-commit`,
then `chmod +x .git/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
set -euo pipefail
files=$(git diff --cached --name-only --diff-filter=ACM \
        | grep -E '\.(py|pyi|ts|tsx|js|mjs|cjs|jsx|go|java|cs|php|swift|kt|dart)$' || true)
[ -z "$files" ] && exit 0
echo "$files" | xargs -I{} aegis check "$OLDPWD/{}"
```

Verify by triggering a known-fail (paste this into the user's repo
to confirm the hook works, then revert):

```bash
echo "def add(a, b)" > /tmp/aegis_test_broken.py
cp /tmp/aegis_test_broken.py ./broken_test.py
git add broken_test.py
git commit -m "should be blocked"   # expect: rejected with [Ring 0] message
git restore --staged broken_test.py
rm broken_test.py
```

Reference: [`docs/integrations/git_pre_commit.md`](docs/integrations/git_pre_commit.md)
(read this if the user asks for variations).

### Path B — GitHub Action

Add `.github/workflows/aegis.yml` to the user's repo. The full
workflow is in [`docs/integrations/github_action.md`](docs/integrations/github_action.md);
copy it verbatim. Then tell the user to mark the "Aegis Ring 0"
check as required in branch protection (you can't do this for them
— it's a GitHub UI action).

### Path C — Wrap the user's LLM in `LLMGateway`

As of V1.10, the integration is the `aegis pipeline run` binary
subcommand or the `aegis-mcp` server. There is no longer a Python
in-process API to wrap — the agent invokes the binary as a child
process or talks to `aegis-mcp` over stdio.

```bash
# Drive a single multi-turn pipeline run.
export AEGIS_PROVIDER=openai          # or openrouter | groq
export OPENAI_API_KEY=...             # or AEGIS_API_KEY / OPENROUTER_API_KEY / GROQ_API_KEY
aegis pipeline run \
  --task "rename the foo helper to bar across the codebase" \
  --root . \
  --max-iters 3 \
  --json
```

For LLM clients with their own loop (Cursor / Claude Code / custom
agent), use the MCP server — `cargo install --path crates/aegis-mcp`
then add `aegis-mcp` to the MCP client config per
[`docs/integrations/mcp_design.md`](docs/integrations/mcp_design.md).
The agent calls `validate_change(path, new_content, old_content?)`
mid-loop and gets a structured verdict back. Only `validate_change`
exposed (intentionally narrow surface — if the user needs the other
tools, tell them to file an issue).

---

## Rules you must follow when working on / with Aegis

These are the project's framing constraints. PRs that violate them
get reverted; suggestions that violate them get rejected. **Do not
propose any of these to the user.**

1. **Do not add automatic retry.** If `gateway.generate_and_validate()`
   raises, the agent / human decides what to do next. Aegis does not
   loop until it gets a passing response.
2. **Do not feed verifier output back into the LLM prompt.** The
   verdict is observation, not coaching. If the user wants the LLM
   to "know what was wrong", they edit the prompt themselves —
   you do not auto-construct hint strings from `gateway.last_trace`.
3. **Do not propose new gates / decision patterns / verifier types
   without checking [`docs/post_launch_discipline.md`](docs/post_launch_discipline.md).**
   That file lists 5 explicit categories of features deferred until
   real-user demand justifies them.
4. **Do not propose extracting `aegis-core` as a generic framework.**
   See [`docs/future_abstraction.md`](docs/future_abstraction.md) —
   three trigger conditions must be met first; they aren't yet.
5. **Do not modify `crates/aegis-decision/src/task.rs::TaskVerdict`** to add
   feedback / hint / next_plan / advice / guidance fields. The cargo
   contract test `crates/aegis-decision/tests/contract.rs` enforces
   rule 2 structurally.

If a user request implies any of these, stop and explain that the
request would change Aegis from a constraint system into an optimizer,
and ask whether they want a discussion thread instead of a PR.

---

## Where things are

Cheatsheet for "I need to find X in this repo":

| You need... | Look at... |
| :--- | :--- |
| Understand what Aegis is for humans | [`README.md`](README.md) |
| Single-file static analysis (Ring 0 + 0.5) | `aegis check <files>` (or `--json`) |
| List supported source languages | `aegis languages` (or `aegis languages --json`) |
| Drive an LLM-backed multi-turn refactor | `aegis pipeline run --task "..." --root . [--scope sub] --max-iters N` (env: `AEGIS_PROVIDER`, `AEGIS_MODEL`, `AEGIS_API_KEY`) |
| MCP server for Cursor / Claude Code | `aegis-mcp` (stdio JSON-RPC; protocol `2025-06-18`) |
| LLM-provider trait + first impl | [`crates/aegis-providers/src/lib.rs`](crates/aegis-providers/src/lib.rs), [`openai.rs`](crates/aegis-providers/src/openai.rs) |
| Add a new LLM provider | mirror `OpenAIChatProvider`; implement the `LLMProvider` trait |
| Add a new source language | one Cargo dep + one [`crates/aegis-core/src/ast/languages/<lang>.rs`](crates/aegis-core/src/ast/languages/) adapter + one [`crates/aegis-core/queries/<lang>.scm`](crates/aegis-core/queries/) query, then register in [`registry.rs`](crates/aegis-core/src/ast/registry.rs). Per-language checklist in [`docs/multi_language_plan.md`](docs/multi_language_plan.md). |
| Run all tests | `cargo test --workspace` |
| Cross-model sweep | (script not yet ported; see V1.8 in [`docs/v1_rust_port_plan.md`](docs/v1_rust_port_plan.md) — the harness is gated on API quotas, not code) |
| Understand the V1.6 evidence | [`docs/v1_validation.md`](docs/v1_validation.md) |
| Understand what V2 looks like | [`docs/gap3_control_plane.md`](docs/gap3_control_plane.md) (design only, not implemented) |
| Understand what's deferred | [`docs/post_launch_discipline.md`](docs/post_launch_discipline.md) |

---

## When things go wrong

If a user reports something Aegis blocks that they think shouldn't
be blocked:

1. Run `aegis pipeline run --task "..." --root . --json` (or
   `aegis check <files> --json`) to capture the full per-iteration
   trace / signals.
2. Identify which gate fired (the `layer` field).
3. **Don't** propose modifying the gate logic to let it pass.
4. **Do** open an issue at https://github.com/wei9072/aegis/issues
   with: their input, the trace, and why they think the block was
   wrong. This is V0.x evidence — false positives + false negatives
   both refine the system.

If a user reports Aegis didn't block something it should have:

1. Same — capture the trace and what they expected.
2. **Don't** propose adding a new gate inline.
3. **Do** open an issue. The fix path is "add a scenario that
   reproduces this, then design the gate that catches it" — not
   reactive patches.

---

## Status (so you don't promise things that aren't real)

What works in V0.x: [`README.md`'s Status section](README.md#status).
Don't promise the user features marked 🟡 or ❌ — they don't exist
yet, and "I'll just build a quick version" is exactly what
[`docs/post_launch_discipline.md`](docs/post_launch_discipline.md)
is meant to prevent.

If you're unsure whether a capability exists: grep the codebase
before claiming it does. The code is the source of truth; this
file and README can drift, the code can't.
