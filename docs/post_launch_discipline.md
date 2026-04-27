# Post-launch discipline — what Aegis does NOT do (yet)

Aegis V1.10 ships as a single Rust workspace, zero Python at
runtime. The design philosophy is sharp, the mechanism is shipped,
the Python-era V1.6 evidence is preserved in
[`v1_validation.md`](v1_validation.md). The hard part now is
**resisting the urge to add features** before the framework gets
validated by real-world use.

This document is the public hill: features and changes that *will be
declined* in PRs, with the reasoning. Contributors proposing them
should be redirected here first; the issue can re-open after a
genuine case (real user blocked by absence of the feature) emerges.

The discipline is from a roadmap conversation summarised as:

> **Aegis is not a product that needs to be protected.**
> **It is an abstraction that needs to be validated.**
>
> Now is not the time to extend functionality. Now is the time to
> let the system enter the real world and collect behaviour
> evidence.

---

## What is being deferred (and why)

### 1. No new features

The 6 surviving gates (Ring 0 syntax, Ring 0.5 fan-out + chain
depth, cost regression, PlanValidator, Executor + Snapshot,
Stalemate / Thrashing detector) + Layer C verifier contract +
V1.5 5-family evidence + V1.6 stalemate verification ship one
**complete, testable mechanism**. Adding new gate types, new
decision patterns, new verifier classes, etc. *before* external
users have stress-tested the existing surface produces:

- **Premature complexity** — features no real user asked for, but
  whose maintenance cost is real.
- **Over-fit framework** — design decisions made to satisfy
  speculative use cases that wouldn't survive contact with the
  second domain.

PRs that add new functionality will be evaluated against the
question: *did a real user hit a wall that this PR removes?* If
the answer is "this would be useful for hypothetical X", the PR
should become a discussion thread instead.

### 2. No domain-agnostic framework extraction

[`docs/future_abstraction.md`](future_abstraction.md) documents the
framework-vs-plugin split that *will eventually* exist (the
code-generation specifics in `crates/aegis-runtime` and
`crates/aegis-providers` separate from the truly domain-independent
loop primitives in `crates/aegis-trace` + `crates/aegis-decision`).
It also documents the three trigger conditions that must **all**
be met before extraction starts:

1. V2 feature-complete (Gap 3 implemented; Adaptive Policy at least
   prototyped)
2. At least one second-domain pilot exists (database migration,
   canary deploy, etc.)
3. The pilot exposed ≥1 friction point telling us which abstraction
   is wrong

Until all three trigger, **no separate `aegis-framework` crate, no
plugin registry, no domain-agnostic public API**. The current
crate split (`aegis-trace` / `aegis-decision` / `aegis-runtime`
/ etc.) already telegraphs the future shape; extracting earlier
would freeze accidental code-gen specifics into the framework
contract.

### 3. No policy learning / scoring / trust score

Adaptive Policy — a trust-score system that would learn which
decision patterns to trigger differently based on past behavior —
is **explicitly off the roadmap**. It would shift Aegis from
"rejector" to "optimizer". See [ROADMAP.md](ROADMAP.md)'s
"Explicitly NOT on the roadmap" section.

PRs that add learned policy / scoring / ranking layers will be
declined regardless of how much data accumulates. The framing is
the constraint.

### 4. No SDK / REST API / UI / dashboard

Aegis V1.10 has two surfaces: the `aegis` CLI binary and the
`aegis-mcp` stdio MCP server. That's it.

A polished SDK across multiple languages, a hosted REST API for
remote callers, a UI for browsing decision traces, a dashboard
for run history — all are reasonable products to build *on top of*
Aegis. They are **not Aegis itself**. They live in separate
projects that shell out to `aegis` or talk to `aegis-mcp` over
stdio.

If a consumer wants Aegis exposed via REST or a UI, that's a
wrapper repository they own. The Aegis core stays as a Rust
binary + MCP server only.

### 5. No auto-retry / verifier-feedback loop

This one is non-negotiable. From
[`docs/gap3_control_plane.md`](gap3_control_plane.md)'s Critical
Principle:

> The control layer must not become a retry engine.

The four anti-patterns documented in that file:

- ❌ `if verifier_failed: pipeline.run(...)` — automated retry
- ❌ `if stalemate_detected: hint = generate_hint(...); pipeline.run(task + hint)` — system-generated coaching
- ❌ `for attempt in range(N): pipeline.run(...)` — retry-until-success loop
- ❌ Controller consumes `verifier_result.evidence` and adjusts the next prompt

Any of these turns Aegis from a decision system that judges its own
work into an optimizer that bends invariants in pursuit of
satisfying an external goal. **That's the opposite of why Aegis
exists.**

Test enforcement: `crates/aegis-decision/tests/contract.rs`
fails on any `TaskVerdict` field name containing `retry / feedback
/ hint / advice / guidance`, and pins `TaskVerifier` as a
single-method trait. PR authors who want to weaken these tests owe
a framing-level conversation before the merge.

---

## What IS encouraged right now

The opposite list — things actively wanted from external
contributors:

- **Integration examples** ([issue draft](launch/issue_integration_examples.md))
  — Aegis embedded inside an existing agent / CI / IDE plugin.
- **Thrashing case evidence** ([issue draft](launch/issue_thrashing_call.md))
  — real `aegis pipeline run` traces producing `THRASHING_DETECTED`.
- **Cross-platform install reports** — specific OS + workflow
  reports of where `cargo install --path crates/aegis-cli` fails.
- **New language adapters** — one Cargo dep + one adapter file
  under `crates/aegis-core/src/ast/languages/` + one `.scm` query.
  Per-language checklist is in
  [`multi_language_plan.md`](multi_language_plan.md).
- **Documentation fixes** — anything in `docs/` or `README.md` that
  misrepresents the current state.
- **Provider additions** — new `LLMProvider` impl in
  `crates/aegis-providers/` for a model family not yet covered
  (e.g. Anthropic, Mistral, Cohere). Mirror `OpenAIChatProvider`.

---

## When this document gets revisited

Reload the discipline list when any of:

- A real user files an issue saying "I can't do X with Aegis" — that's
  evidence one of the deferred items might now be justified.
- Two or more independent users hit the same friction — the deferred
  item moves up in priority.
- The trigger conditions in [`future_abstraction.md`](future_abstraction.md)
  fire — that's the signal to start framework extraction.

Until any of those happen, the discipline above stands. The bar for
adding to Aegis right now is not *"is this useful?"* — it's
*"is this useful, requested by a real user, AND consistent with the
negative-space framing?"*. All three conditions, not any.
