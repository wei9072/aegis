# V3 — aegis-agent design

> **Status:** Plan + contract tests shipped (2026-04-27). Implementation
> phases V3.0–V3.7 follow. This document is the single source of truth
> for the V3 design rationale; if implementation diverges, update this
> doc in the same commit.

---

## What V3 is

A coding agent (`aegis-agent` crate + binary) built on aegis primitives
as a library. The agent borrows conversation / tool / API / session /
hook scaffolding from [claw-code](https://github.com/ultraworkers/claw-code)
(MIT) and adds **four aegis-specific differentiation points** that no
other coding agent has today.

Two-mode framing:

- **Substrate mode** (V3 core): aegis primitives are a sensor library
  the agent queries during its own thinking. aegis itself is unchanged
  — it still only judges, never directs.
- **Side-channel mode** (V1.10 existing): `aegis-mcp` + PreToolUse hook
  wrap an external agent (Claude Code, Cursor) — agent doesn't know it
  exists, just gets verdicts back.

Both modes coexist. Substrate is for users who want an agent built
ground-up around architectural safety; side-channel is for users who
already have a coding agent and want a brake.

---

## Why this and not the alternatives

Three alternatives were considered and rejected:

### A. Don't build an agent — keep V2 release polish on track
**Why rejected:** the user pivoted explicitly. Side-channel mode covers
80% but `aegis pipeline run` is too primitive for "give it a ticket and
walk away" scenarios. Without an agent surface, aegis cannot serve the
agentic-development use case it claims to support.

### B. Fork claw-code and inject aegis as the gate layer
**Why rejected:** claw-code is in active multi-author development;
fork divergence cost is real. claw-code's runtime crate has high
internal coupling, hard to inject aegis at the right hook points
without rewriting big chunks. MIT license allows copy-and-modify
which gets us 90% of the leverage without the divergence pain.

### C. Implement aegis-agent matching `docs/AEGIS_HARNESS_ARCHITECTURE.md`
**Why rejected:** that document (in the claw-code repo) describes a
different "Aegis" than ours — its 5-subsystem architecture includes
**Auto-Healing Loop**, which violates the negative-space framing
(`gap3_control_plane.md` Critical Principle). Same name, different
designs. Following that spec would require tearing up the framing
discipline we've been defending.

**Chosen path:** copy-and-modify claw-code parts, write only the four
aegis-specific hook points, structurally enforce the framing red lines
with contract tests so future PRs cannot quietly drift into auto-healing.

---

## The four differentiation points

Where aegis-agent diverges from "just another claw-code clone". All four
hook into the existing claw-code conversation skeleton — none requires
rewriting the main loop.

### 1. PreToolUse aegis-verdict prediction

**Insertion point:** the existing PreToolUse hook stage in
`conversation::run_turn`.

**Behaviour:** when the LLM proposes a tool call (Edit / Write /
MultiEdit), the agent calls `aegis-core` to compute the verdict the
write would receive *before* executing it. If the verdict is BLOCK,
the tool call is rejected — the LLM gets `ToolError` and decides
what to do next.

**vs claw-code:** claw-code only has permission allow/deny gating
(read-only / workspace-write / danger-full-access). aegis-agent adds
**structural prediction** — it self-rejects plans the agent itself
knows would fail aegis.

**vs Claude Code + aegis-mcp:** the MCP-mode equivalent is
`mcp__aegis__validate_change`, which the LLM calls voluntarily.
Substrate mode makes the prediction **mandatory** — wired into the
hook, not the prompt.

### 2. Cross-turn structural cost tracking

**Insertion point:** end of each turn in `conversation::run_turn`,
after `pending_tool_uses.is_empty()`.

**Behaviour:** record the current cost of all touched files into
session metadata (NOT into the LLM-visible message history).
Compare against session start; if cumulative regression exceeds
`session_cost_budget`, terminate the session with
`StoppedReason::CostBudgetExceeded`.

**vs claw-code:** completely absent. claw-code tracks token usage,
not structural cost.

**vs Claude Code + aegis-mcp:** MCP is per-call stateless — it can
catch a single edit going from cost 18 → 22, but cannot catch five
edits of +1 each that compound to +5 within session budget. Substrate
mode owns the cross-turn view.

### 3. Verifier-driven done

**Insertion point:** when `pending_tool_uses.is_empty()` (LLM
emitted no more tool calls — its way of saying "done").

**Behaviour:** if a verifier is configured, run the verifier suite
(test / build / structural target / shell). The verifier's verdict
is the source of truth on whether the turn truly completed.
- `SOLVED` → `StoppedReason::PlanDoneVerified`
- `INCOMPLETE` → `StoppedReason::PlanDoneVerifierRejected`
- No verifier → `StoppedReason::PlanDoneNoVerifier`

The verdict goes to `AgentTurnResult.task_verdict` for the user to
read. **It is never converted to a hint string and injected into a
follow-up prompt** — that's `no_coaching_injection.rs`'s job to enforce.

**vs claw-code:** claw-code trusts the LLM's "no more tool_use"
unconditionally. Anthropic itself reports overly-generous self-evaluation
as a known LLM failure mode.

**vs Claude Code + aegis-mcp:** Stop hook can run `aegis verify` after
Claude Code's session ends, but the verdict has nowhere to go (Claude
Code is already finished). Substrate mode integrates verifier into the
agent's own done-decision loop.

### 4. Stalemate / thrashing detection at session level

**Insertion point:** every turn end. Reuses
`aegis_runtime::loop_step::step_decision`, treating turns as
iterations.

**Behaviour:** if cost stays flat across N turns → `StalemateDetected`.
If cost oscillates → `ThrashingDetected`. Either case terminates the
session with the named reason. **No retry is triggered — the user
decides whether to start a new session with a refined task.**

**vs claw-code:** absent. claw-code has `max_iterations` per turn
(retry budget) but no cross-turn convergence detection.

**vs Claude Code + aegis-mcp:** MCP cannot see Claude Code's loop;
this would require MCP session memory (which doesn't exist in V1.10).
Substrate mode owns the loop directly so the detection is trivial.

---

## What the four points are NOT

This section exists because each of these is one merge away from
becoming auto-healing if not stated explicitly:

| Tempting feature | Why it's banned |
|---|---|
| "On `PlanDoneVerifierRejected`, automatically rerun the turn with the verdict prepended to the next prompt" | This is the auto-retry engine. Banned by `no_auto_retry.rs` and `no_coaching_injection.rs`. |
| "When `StalemateDetected` fires, generate a hint about which signal isn't moving and inject it" | Same — `no_coaching_injection.rs`. |
| "Build a `RetryPromptBuilder` that takes a `TaskVerdict` and produces a follow-up `system` message" | Type-name-banned by `no_coaching_injection.rs`. |
| "Add `auto_retry: bool` to `AgentConfig` for users who *want* it" | Field-name-banned by `no_auto_retry.rs`. The framing isn't a config knob. |

The contract tests under `crates/aegis-agent/tests/` are the trip-wires.
Touching any of them in a PR forces a framing-level conversation
before the merge.

---

## What we borrow from claw-code

`claw-code/rust/crates/runtime/` and `crates/api/` ship roughly
**11k LOC** that aegis-agent doesn't need to re-derive:

| claw-code module | LOC | What it gives us |
|---|---|---|
| `runtime/conversation.rs` | 1811 | Multi-turn loop + tool dispatch + hook attachments + auto-compaction |
| `runtime/file_ops.rs` | 839 | Read/Write/Edit/Glob/Grep with binary detection, size limits, workspace boundary |
| `runtime/bash.rs` + `bash_validation.rs` | ~1300 | Bash exec + sandbox + dangerous-command attenuation |
| `runtime/session.rs` + `compact.rs` | ~1800 | Session persistence + auto-summarisation |
| `runtime/permissions.rs` + `permission_enforcer.rs` | ~1300 | read-only / workspace-write / danger-full-access modes |
| `runtime/hooks.rs` | 1116 | PreToolUse / PostToolUse hook wiring |
| `runtime/mcp_*.rs` | ~2000 | MCP **client** (so aegis-agent can call external MCP servers, including aegis-mcp itself) |
| `api/` (anthropic + openai-compat + xai + dashscope + sse + prompt cache) | ~2200 | Multi-provider streaming |

**License:** claw-code is MIT. Borrowing means **copy-and-modify**
into `crates/aegis-agent/src/` with attribution comments at the top of
each lifted file (form: `// Adapted from claw-code (MIT) — <upstream path>`).

**What we do NOT borrow:** `runtime/policy_engine.rs` (claw-code's
path/cmd attenuation). aegis has its own `PlanValidator` + 6 layers
that already cover this — and replacing claw-code's policy engine
with aegis's gates is one of the four differentiation points.

---

## Phase status (as shipped — all V3 phases ✅ Done 2026-04-27)

| Phase | Content | Status |
|---|---|---|
| **V3.0** — Skeleton + contract tests | Crate scaffolding; 9 framing contract tests | ✅ Done |
| **V3.1a** — Conversation skeleton | ConversationRuntime, ApiClient + ToolExecutor traits, message types, scripted stubs (adapted from claw-code) | ✅ Done |
| **V3.1b** — OpenAI-compat provider | HttpClient abstraction + OpenAiCompatProvider (covers OpenRouter / Groq / Ollama / vLLM / llama.cpp / LMStudio / DashScope) | ✅ Done |
| **V3.2a** — Anthropic Messages provider | Distinct wire format (top-level `system`, content blocks, `x-api-key` auth, `anthropic-version` header, flat tool definitions) | ✅ Done |
| **V3.2b** — MCP client | JsonRpcTransport trait + StdioTransport / ScriptedTransport; McpClient (initialize/tools/list/tools/call); McpToolExecutor wraps as ToolExecutor; verified against real `aegis-mcp` binary | ✅ Done |
| **V3.2c** — Gemini provider | `generateContent` format with `model` role, `parts[]`, `functionCall`, `systemInstruction`, `x-goog-api-key` | ✅ Done |
| **V3.3** — Differentiation A + B | PreToolUsePredictor trait + AegisPredictor (calls aegis-mcp validate_change before file-write tools); CostTracker with cumulative regression + CostBudgetExceeded termination | ✅ Done |
| **V3.4** — Differentiation C | AgentTaskVerifier trait + ShellVerifier / TestVerifier / BuildVerifier / CompositeVerifier; verifier overrules LLM-claimed done | ✅ Done |
| **V3.5** — Differentiation D | StalemateDetector at session level (3 successive identical cost totals → StalemateDetected) | ✅ Done |
| **V3.6** — Permissions + hooks | PermissionPolicy with three modes (ReadOnly / WorkspaceWrite / DangerFullAccess); PreToolUseHookPredictor (Claude-Code-compatible exit-2 = block) | ✅ Done |
| **V3.7** — Session + compaction + chat_demo | Session serde + atomic save_to / load_from; compact_drop_oldest; chat_demo example wiring all 3 providers | ✅ Done |
| **V3.8** — `aegis chat` CLI + REPL | `aegis chat` subcommand (one-shot / pipe / interactive REPL auto-detect via `IsTerminal`); markdown render + spinner + rustyline editor + slash-command tab-complete (adapted from claw-code's render.rs / input.rs); `/reset` clears session via reset_session(); ReadOnlyTools (Read/Glob/Grep) + `--tools` flag; MultiToolExecutor for combining sources; `--mcp` flag mounts MCP servers as additional tool sources; OpenAI-compat SSE streaming via `stream_with_callback`; per-event REPL rendering with markdown finalisation | ✅ Done |

**Final delivery:** ~7400 LOC (~4200 src + ~3200 tests), 295 cargo
tests across the workspace, 11 attribution-comment-marked files
adapted from claw-code (MIT). Total wall time: one day.

The aggressive timeline only worked because claw-code (MIT) gave us
**~11k LOC** of conversation / tool / api / session / hook
scaffolding to borrow. Building from zero would still be the
6+ month estimate.

Real-LLM dogfood evidence: pending — see
[`v3_dogfood.md`](v3_dogfood.md). The mechanism shipped; the claim
that the four differentiation points actually surface in real
agentic-development sessions will be added once the project owner
runs against a real workspace + real provider.

---

## Contract tests as the framing fence

`crates/aegis-agent/tests/` ships three contract tests *before any
implementation borrowing happens*:

| Test file | What it pins | Tests |
|---|---|---|
| `no_auto_retry.rs` | `AgentConfig` and `AgentTurnResult` cannot grow retry-shaped fields; source has no `fn auto_retry` etc. | 4 |
| `verifier_drives_done.rs` | `StoppedReason` must distinguish `PlanDoneVerified` / `PlanDoneVerifierRejected` / `PlanDoneNoVerifier`; cannot collapse to a generic `PlanDone`; result must carry `task_verdict` field | 4 |
| `no_coaching_injection.rs` | Source has no `VerdictCoach` / `FeedbackInjector` / `fn prompt_from_verdict` etc. | 1 |

**Originally 9 tests; all pass on the empty scaffold.** As V3.1–V3.8
landed, the source-text scans in `no_auto_retry.rs` and
`no_coaching_injection.rs` got upgraded to walk the entire `src/`
directory (rather than just `lib.rs`) so newly-added modules can't
silently introduce a forbidden token. The trip-wire grows with the
crate; PR review then becomes: "the test failed — is the test wrong,
or is your change violating the framing?" That conversation is what
the trip-wire exists to surface.

These complement the existing
`crates/aegis-decision/src/task.rs::tests::task_verdict_has_no_feedback_field`
(the V1 framing fence on `TaskVerdict`); together they enforce the
critical principle from
[`gap3_control_plane.md`](gap3_control_plane.md) at both the
decision-data level and the agent-API level.

---

## Crate dependency graph (after V3 lands)

```
                                ┌──────────────┐
                                │ aegis-agent  │   (V3 — coding agent)
                                └──┬───────────┘
                                   │
                  ┌────────────────┼────────────────┬─────────────────┐
                  ▼                ▼                ▼                 ▼
          ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
          │ aegis-runtime│ │ aegis-providers│ │ aegis-decision│ │ aegis-core  │
          │ (loop, val,  │ │ (LLM clients) │ │ (DecisionPattern│ │ (Ring 0/0.5,│
          │  exec, snap) │ │               │ │  TaskVerifier) │ │  signals)   │
          └──┬───────────┘ └──┬────────────┘ └──┬─────────────┘ └──┬──────────┘
             │                │                 │                  │
             └────────────────┴─────────────────┴──────────────────┘
                                    │
                                    ▼
                            ┌──────────────┐
                            │ aegis-trace  │  (DecisionEvent — pure data)
                            └──────────────┘

   aegis-cli ──→ all of the above (CLI binary)
   aegis-mcp ──→ aegis-core, aegis-runtime (stdio MCP server, side-channel)
```

aegis-agent depends on the full stack but adds no new fundamental
abstractions — its job is **composition**, not invention.

---

## Open questions (resolve as V3 progresses)

1. **Binary or subcommand?** `aegis-agent` as its own binary, or
   `aegis chat` subcommand on the existing `aegis` CLI? Current
   guess: subcommand, to keep one tool one binary.

2. **Conversation transcript persistence schema.** Use claw-code's
   session JSON format verbatim or define a new aegis-specific
   schema? Lean toward verbatim (so claw-code session-debugging
   tools work on aegis-agent transcripts).

3. **Default model.** claw-code defaults to `claude-sonnet-4-6`.
   We should default to whatever the user has API key for —
   detect by env vars, no hardcoded model. Mirrors how
   `aegis pipeline run` works today.

4. **Should V3.0 ship a Stop-hook contract for Claude Code?** The
   side-channel "Stop hook → aegis verify" path mentioned in
   recent design discussion is independent of substrate-mode
   aegis-agent. Could ship in parallel as a small V2.x or V3.0
   companion.

---

## Plan-document maintenance

This document is the canonical source for V3 design. If reality
diverges (a phase turns out harder, a borrowed claw-code module
doesn't fit, a new differentiation point emerges), **update this
document in the same commit that addresses the divergence**.

PRs that change V3 implementation but not this document on substantive
matters will be asked to update the plan first.

The contract tests are the structural floor. This document is the
prose explaining *why* those tests exist. Both must stay in sync.
