# Aegis Roadmap

> Aegis is not a coding assistant.
> Aegis is a **deterministic rejection layer** for LLM-driven code generation.
>
> Correctness is **decision correctness**, not output correctness:
> bad transitions don't stick, good transitions are not directed.

---

## Where Aegis is today (2026-04-27)

V1.10 shipped. The codebase is a single Rust workspace with **zero
Python at runtime**, producing two binaries:

- `aegis` — CLI: `check` / `languages` / `pipeline run`
- `aegis-mcp` — MCP stdio server, one tool: `validate_change`

For the full per-phase port history (V1.0 → V2.0), see
[`docs/v1_rust_port_plan.md`](v1_rust_port_plan.md).

### What ships in V1.10

| Layer | What it does | Where it runs |
| :--- | :--- | :--- |
| **Ring 0** — syntax | tree-sitter parse; `ERROR` / `MISSING` node → BLOCK | `aegis check`, MCP, pipeline |
| **Ring 0.5** — structural signals | `fan_out` (unique imports) + `max_chain_depth` (longest method chain) | `aegis check`, MCP, pipeline |
| **Cost regression** | `sum(signals_after) > sum(signals_before)` → BLOCK / rollback | MCP (when `old_content` given), pipeline |
| **PlanValidator** | path safety / scope / dangerous_path / virtual-FS simulation | `aegis pipeline run` only |
| **Executor + Snapshot** | atomic apply with backup-dir rollback | `aegis pipeline run` only |
| **Stalemate / Thrashing detector** | sequence-level; halts the loop with a named reason | `aegis pipeline run` only |

11 source languages registered (Python, TypeScript, JavaScript, Go,
Java, C#, PHP, Swift, Kotlin, Dart, Rust). See README "Supported
source languages" for per-language status.

### What got deleted in V1.10 (and won't come back unless real users hit a wall)

| Layer (V0.x) | Why deleted | Comeback condition |
| :--- | :--- | :--- |
| **PolicyEngine** (YAML rule table over signals) | Tells the system how to *direct* outcomes — violates negative-space framing | Never (deliberate) |
| **DeliveryRenderer** (warning vs code-channel separation) | No real user; abstraction without consumer | Never absent demand |
| **ToolCallValidator T1** (claim vs reality) | Could come back as ~50 LOC if real "LLM-said-X-but-did-Y" failures emerge | Concrete failure case |
| **ToolCallValidator T2** (semantic comparison) | Costs an extra LLM call per turn; speculative without evidence | Multiple T1 misses + budget |
| **IntentClassifier** | Needs LLM judge; speculative | Real intent-bypass evidence |
| **IntentBypassDetector** | Same as above | Same |

Net effect: V1.10 is **leaner and more honest about what it does**.
The 6 surviving layers all sit cleanly inside the negative-space
framing (each is a rejection valve, none is a goal-direction signal).

---

## V3 — substrate + hand (REMOVED 2026-04-29)

V3 (`aegis-agent`) was a coding agent built on aegis primitives.
All eight phases (V3.0 through V3.8) shipped in one day on
2026-04-27, adding 295 tests to the workspace.

On 2026-04-29 the crate was removed to keep this repository
focused on the control surface — aegis is the harness, not the
agent. Pre-removal V3 codebase is recoverable at tag
[`v0.1.1-pre-extract`](https://github.com/wei9072/aegis/tree/v0.1.1-pre-extract).

The four aegis-specific differentiation patterns V3 explored
remain valid for any future proposer-side implementation:
1. PreToolUse aegis-verdict prediction (agent self-rejects bad plans)
2. Cross-turn structural cost tracking
3. Verifier-driven done (LLM cannot single-handedly claim "done")
4. Stalemate / thrashing detection at session level

---

## V2 release artifacts

V2.0 templates (cross-platform release workflow, Homebrew formula,
npm wrapper) are committed under `.github/workflows/` + `packaging/`.
With V3 removed, the activation gate that previously blocked them
no longer applies. Activation steps remain mechanical:

1. `git tag v0.1.1 && git push origin v0.1.1` — triggers cross-platform build
2. Create `wei9072/homebrew-aegis` tap repo + paste formula + fill sha256s
3. `npm publish --access public` from `packaging/npm/`
4. `cargo publish` each crate in dep order

---

## Other outstanding (unchanged from earlier roadmap)

### V1.8 — cross-model re-validation on the Rust pipeline (gated on API budget)

The V1 + V1.5 + V1.6 sweep evidence in
[`docs/v1_validation.md`](v1_validation.md) was collected on the
Python pipeline (152 multi-turn runs across 5 model families).

V1.8 re-runs the same scenarios against the Rust `aegis pipeline
run` to confirm the framework is implementation-independent.

**Status:** code path works end-to-end on real LLMs; gated on the
user having LLM API budget for ~70 minutes of wall-clock per
sweep matrix. Not a code task.


---

## Backlog (evidence-gated)

These are recorded so that PRs proposing them get a structured
"yes/no" rather than ad-hoc reasoning. **None of them get built
without a real user reporting a real wall.** Bar from
[`post_launch_discipline.md`](post_launch_discipline.md): useful
**AND** requested by real user **AND** consistent with negative-space
framing. All three.

### Layer ports back from V0.x (only if specific failures emerge)

| What | Cost | Trigger |
| :--- | :--- | :--- |
| ToolCallValidator T1 (path-mismatch) | ~50 LOC, 1 afternoon | Real "LLM said wrote X but actually wrote Y" failure |
| IntentBypassDetector | Variable, depends on intent extraction | Real intent-bypass evidence in production trace |

### New capabilities

| What | Why deferred | Trigger |
| :--- | :--- | :--- |
| Per-language `max_chain_depth` overrides (Java, Dart) | Default walker under-counts on these — flagged 🟡 in README | A user complains the chain-depth signal is wrong on their codebase |
| Cross-edit regression detection in MCP/hook mode | Currently each Edit is judged individually; LLM can do 5 separately-OK edits that compound to bad. Needs MCP-mode session memory across calls. | Empirical case where this matters from a real user on Cursor / Claude Code / Aider |
| `aegis sweep` subcommand | Replaces `scripts/v1_validation.py`; needed to run V1.8 in batch | When V1.8 sweep starts |
| Per-language tree-sitter grammar bumps to 0.22+ | Kotlin / Dart pinned to old crates because of ABI mismatch | If grammar quality on those languages becomes a problem |
| `cyclic_dependency` Ring 0 signal | Petgraph + import-query, ~150 LOC. V0.x designed but never shipped | Any time — clean structural signal, passes negative-space check |
| `cognitive_complexity` Ring 0.5 signal | Per-function AST walk, ~200 LOC + 10 `.scm` queries. SonarSource's definition is the cross-language standard | Any time — same rationale |
| `LLMJudgeVerifier` for SRP / cohesion / SoC checks | One LLM call per verify; subject to its own self-eval bias (smaller than first-LLM bias, but real) | A real user reports verifier coverage gap on the `TaskVerifier` trait in `aegis-decision` |

### Explicitly NOT on the roadmap

These are **deliberate non-goals**, not "haven't gotten to them
yet". See [`post_launch_discipline.md`](post_launch_discipline.md)
for the full list and rationale.

- **PolicyEngine / rule-table over signals** — directs toward goals
- **Adaptive policy / learned thresholds** — needs training data, but
  more importantly, would shift Aegis from "rejector" to "optimizer"
- **HITL implementation** — design pinned in
  [`gap3_control_plane.md`](gap3_control_plane.md), implementation
  gated on real user need
- **Web UI / dashboard / hosted SaaS** — these belong in wrappers
  built **on top of** Aegis, not in Aegis itself
- **Auto-retry / verifier-feedback loop** — non-negotiable hill;
  structurally enforced by
  `crates/aegis-decision/src/task.rs::tests::task_verdict_has_no_feedback_field`.
  The V3 era added three additional contract tests
  (`no_auto_retry.rs`, `verifier_drives_done.rs`,
  `no_coaching_injection.rs`) which were removed alongside the
  `aegis-agent` crate on 2026-04-29; if a future proposer-side
  implementation is built, the same contracts should travel with it.

---

## Guiding principle

> **Is this rejecting degradation, or directing toward a goal?**

Every roadmap item must answer "rejecting degradation". The latter
is the wrong direction for Aegis, and rejecting it is the entire
reason this project exists.

If a future PR proposes work that fails this check, the right
response is a framing-level conversation, not code review.
