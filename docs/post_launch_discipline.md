# Post-launch discipline — what Aegis does NOT do (yet)

Aegis V0.x is open. The design philosophy is sharp, the mechanism
is shipped, the V1.6 evidence is in. The hard part now is
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

The 8 in-pipeline gates + multi-turn pipeline + Layer C verifier +
V1.1 stalemate / thrashing detection + V1.5 5-family evidence +
V1.6 stalemate verification ship one **complete, testable
mechanism**. Adding new gate types, new decision patterns, new
verifier classes, etc. *before* external users have stress-tested
the existing surface produces:

- **Premature complexity** — features no real user asked for, but
  whose maintenance cost is real.
- **Over-fit framework** — design decisions made to satisfy
  speculative use cases that wouldn't survive contact with the
  second domain.

PRs that add new functionality will be evaluated against the
question: *did a real user hit a wall that this PR removes?* If
the answer is "this would be useful for hypothetical X", the PR
should become a discussion thread instead.

### 2. No abstraction extraction (no `aegis-core` yet)

[`docs/future_abstraction.md`](future_abstraction.md) documents the
framework-vs-plugin split that *will eventually* exist. It also
documents the three trigger conditions that must **all** be met
before extraction starts:

1. V2 feature-complete (Gap 3 implemented; Adaptive Policy at least
   prototyped)
2. At least one second-domain pilot exists
3. The pilot exposed ≥1 friction point telling us which abstraction
   is wrong

Until all three trigger, **no `aegis-core` package, no plugin
registry, no domain-agnostic API**. Extracting earlier freezes
accidental code-gen specifics into the framework contract.

### 3. No policy learning / scoring / trust score

[`ROADMAP.md` §4.3](ROADMAP.md) describes Adaptive Policy — a
trust-score system that would learn which decision patterns to
trigger differently based on past behavior. It's intentionally
deferred to V2+ because it requires *training data* that only
exists once Gap 1, Gap 2, and Gap 3 have collected enough real
traffic.

PRs that add learned policy / scoring / ranking layers before this
data exists will be declined. The data has to come first.

### 4. No SDK / REST API / UI / dashboard

Aegis V0.x has two surfaces: Python library (the product) and CLI
(the demo wrapper). That's it.

A proper SDK across multiple languages, a hosted REST API for
non-Python callers, a UI for browsing decision traces, a dashboard
for run history — all are reasonable products to build *on top of*
Aegis. They are **not Aegis itself**. They live in separate
projects, called by Aegis's API.

If a consumer wants Aegis exposed via REST or a UI, that's a
wrapper repository they own. The Aegis core stays as a Python
library + CLI only.

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

Test enforcement: `tests/test_task_verifier.py::test_task_verdict_has_no_feedback_field`
fails on any field name containing `retry / feedback / hint /
advice / guidance`. PR authors who want to weaken this test owe a
framing-level conversation before the merge.

---

## What IS encouraged right now

The opposite list — things actively wanted from external
contributors:

- **Integration examples** ([issue draft](launch/issue_integration_examples.md))
  — Aegis embedded inside an existing agent / CI / IDE plugin.
- **Thrashing case evidence** ([issue draft](launch/issue_thrashing_call.md))
  — real Aegis runs producing `THRASHING_DETECTED` traces.
- **Build friction reports** ([issue draft](launch/issue_rust_build_friction.md))
  — specific OS + workflow reports of where Rust extension setup
  failed.
- **New scenario contributions** — adding a `tests/scenarios/<name>/`
  directory with input + scenario.py + verifier.py. Even
  scenarios where the model-of-the-day fails are useful, because
  they characterize the failure mode in the decision-pattern
  vocabulary.
- **Documentation fixes** — anything in `docs/` or `README.md` that
  misrepresents the current state.
- **Provider additions** — new `LLMProvider` subclass for a model
  family not yet covered (e.g. Anthropic via the Anthropic SDK,
  Mistral, Cohere, etc.). Mirror the shape of
  `aegis/agents/groq.py`.

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
