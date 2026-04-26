# Aegis launch announcement (draft)

Short version intended for: a Show HN post, a tweet thread (split as
needed), a blog post intro, or the GitHub repo "About" + pinned
README block.

Use as-is or remix; the key claims and tagline should stay. Edit
the personal voice to match yours.

---

## TL;DR (one paragraph)

> **Aegis is a behavior harness for LLM-driven systems. Instead of
> teaching models what is good, it rejects outcomes that make the
> system worse.** This is its first instantiation: a code-generation
> harness that wraps any LLM provider in a control plane that
> observes structural signals, judges its own work, and rolls back
> changes that make code worse — independent of what the LLM thinks
> it accomplished.
>
> Open-sourcing now (v0.x) because Aegis needs validation across
> domains, not protection. Repo: https://github.com/wei9072/aegis

---

## Long version (~6 paragraphs)

### What it is

Most AI coding tools try to make the model produce better code.
Aegis takes the opposite approach: it ensures worse outcomes are not
accepted.

When an LLM proposes a code change, Aegis runs the proposal through
a control plane: structural-signal extraction (Rust-backed), policy
gates (fan-out, dependency cycles, syntax), tool-call validation
(does the LLM's claim match what was actually written?), intent
classification (was this a code-gen request or a teaching request?),
and a multi-turn iteration loop with **cost-aware regression
rollback** — if the post-apply signals are structurally worse than
before, the change is reverted, and the planner is told what got
worse and asked to try a different approach.

Every decision the system makes is named and emitted as a
machine-readable trace event. There are 9 named decision patterns
covering every observable iteration shape (`APPLIED_DONE`,
`REGRESSION_ROLLBACK`, `STALEMATE_DETECTED`, etc.) and 5 task-level
verdicts (`SOLVED` / `INCOMPLETE` / `ABANDONED` / ...). The trace
**is** the product — it lets you build dashboards, audits,
escalation policies, or just `print()` debugging on top of any LLM
agent without modifying the agent.

### Why it's different from Aider / Claude Code / LangSmith

| Tool | What it bets on |
| :--- | :--- |
| Claude Code | I can make the LLM use tools well |
| Aider | I can make the LLM write code obediently |
| LangSmith | I can show you what the LLM is doing |
| **Aegis** | **I ensure worse outcomes are not retained** |

The framing matters because the four bets imply four different
architectures. Aegis sits *between* any coder (LLM or human) and the
codebase, enforcing structural invariants. The Executor is the only
allowed writer; the LLM literally cannot call `write_file`. When the
loop notices itself stuck — same plan repeating, signals not moving,
rollbacks chaining — it terminates with a named reason
(`STALEMATE_DETECTED`, `THRASHING_DETECTED`) instead of silently
exhausting `max_iters`.

### What's validated, what isn't

Aegis V1 + V1.5 + V1.6 ship with empirical evidence across **5
model families** (Google Gemma, Meta Llama 3, OpenAI gpt-oss,
Alibaba Qwen, InclusionAI Ling) — 152 multi-turn sweep runs, full
data in [`docs/v1_validation.md`](docs/v1_validation.md).

**What's directly proven:**

- The decision system fires across all five families. Same gate
  vocabulary, no model-specific exceptions, no `unknown` patterns.
- `REGRESSION_ROLLBACK` triggered in 8 of 15 capable-model runs on
  the regression-designed scenario. Zero false positives across 75
  runs of improving / trivial scenarios.
- Layer C task verifier caught two **INCOMPLETE** cases across two
  model families (LLM declared "done", verifier disagreed,
  workspace was actually still broken). Without Layer C, those would
  have been counted as successes.
- `STALEMATE_DETECTED` fired in 8 of 10 V1.6 verification runs
  across two OpenRouter model families. The pipeline now answers
  "why did it stop?" with a structural reason instead of
  "max iterations".

**What's mechanism-only:**

- `THRASHING_DETECTED` (≥2 consecutive regression rollbacks) is
  shipped and unit-tested but hasn't fired in real-traffic V1.6
  evidence (the verification models got stuck earlier in the loop).
  See the [thrashing-cases issue](./issue_thrashing_call.md) for an
  open call to capture this in real traffic.
- Gap 3 (Human-in-the-loop control plane) has a full design spec
  (`docs/gap3_control_plane.md`) but no code yet. The control
  layer's critical principle — *it must not become a retry engine* —
  is documented as a non-negotiable hill before implementation.

### Why open source now (and what "needs to be validated, not protected" means)

Aegis is at an unusual point: the system works, the framework
philosophy is sharp, but the abstraction has only one user case
(code generation). The framework / plugin split is documented in
[`docs/future_abstraction.md`](docs/future_abstraction.md) but
extraction is **explicitly deferred** until a second domain pilot
exposes the right friction points.

Open-sourcing the codegen instantiation is how the second user case
finds the project. We're not claiming this is a finished framework
or a polished product — we're claiming the abstraction is real
enough to be tested by reality.

> **Aegis is not a product that needs to be protected.**
> **It is an abstraction that needs to be validated.**

If you have:

- An AI coding workflow you'd like to put a "won't accept worse
  outcomes" gate around → try the
  [examples/](examples/) and tell us where it breaks.
- A non-code-gen domain (database migrations, CI canary deploys,
  config rollouts, trading risk) where "propose → validate → apply →
  measure → rollback" is the loop shape → see
  [`docs/future_abstraction.md`](docs/future_abstraction.md) for
  whether the framework would map; we want to talk.
- A real Aegis run where `THRASHING_DETECTED` fires → the
  [thrashing-cases issue](./issue_thrashing_call.md) is the place
  to drop it.

### Caveats up front

This is V0.x. Specifically:

- The Rust extension currently builds via `cd aegis-core-rs &&
  maturin develop --release` — there is no `pyproject.toml` yet, so
  `pip install -e .` from the repo root won't fully Just Work.
  Prebuilt PyPI wheels coming soon.
- The default planner uses Gemini's free Gemma model. Other
  providers (OpenRouter, Groq, generic OpenAI-compatible) are
  supported via `--provider` or by passing the appropriate
  `LLMProvider` instance.
- Three of the eight in-pipeline gates use one extra LLM call each
  (Tier-2 ToolCallValidator, IntentBypassDetector, optional
  semantic comparator). This roughly doubles per-iteration cost on
  top of the planner call. Acceptable for evaluating the framework;
  not yet optimized for production budgets.
- Free tiers of LLM providers vary widely in TPM / quota. Groq's
  free tier is fast but has tight per-minute caps; OpenRouter free
  models route to upstreams that sometimes 429 transiently. Aegis
  classifies these correctly as `ABANDONED` with provider error in
  the rationale — the framework absorbs the constraint, but full
  N-runs sweeps on free tiers will produce mixed evidence.

### Get started

```bash
git clone https://github.com/wei9072/aegis && cd aegis
# see README for build steps
PYTHONPATH=. python examples/02_gateway_single_call.py
```

Repo: https://github.com/wei9072/aegis
Issues / discussions welcome.

---

## Even shorter — Show HN one-liner option

> **Show HN: Aegis — a behavior harness that rejects bad LLM-generated changes (instead of teaching the model to make better ones).** First instantiation is for code; framework extraction waits until a second domain finds us.

## Tweet-length option

> Most AI coding tools try to make the model produce better code.
> @aegis takes the opposite bet: ensure worse outcomes are not
> accepted. Open-sourcing v0.x because the abstraction needs
> validation, not protection. github.com/wei9072/aegis
