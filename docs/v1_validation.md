# V1 validation evidence

This document is the V1 charter close-out: the V1 push shipped the
mechanism (8 in-pipeline gates + DecisionTrace + multi-turn rollback
loop), but the **claim** — "Aegis is a decision-system that observes,
judges, rolls back, and re-plans" — needs evidence across five
orthogonal layers, not just unit tests.

The data here was gathered by `scripts/v1_validation.py` — a sweep of
4 scenarios × 5 runs × 3 model families (60 runs total) — and
aggregated by `scripts/v1_aggregate.py` from
`tests/scenarios/<name>/runs/*.json`. Snapshots from the dev sessions
that *built* V1 are archived under `runs_pre_v1/` so the data here is
exclusively post-V1-mechanism.

Reproducing the sweep:

```bash
PYTHONPATH=. python scripts/v1_validation.py
PYTHONPATH=. python scripts/v1_aggregate.py > docs/v1_validation_runs.md
```

---

## Framing — what Aegis actually is

Before reading the layer evidence, fix the framing — otherwise the
numbers below get interpreted against the wrong yardstick.

> **Aegis does not try to make AI produce better code.**
> **It ensures worse outcomes are not accepted.**

Restated:

- Not "this output is bad, please improve it" — but "this transition
  made the state worse, not accepted, revert to the previous state and
  let the LLM try a different approach."
- Aegis does not define the optimal end state of the code. It only
  defines which state transitions are unacceptable.
- The system's core is **constraint**, not teaching.
  **Exclusion of error paths**, not provision of goals.
- Changes that survive are those not flagged as degradation. Overall
  behavior trends toward improvement, but improvement is the
  *cumulative residue* of "no degradation accepted by default" — not
  a directly optimized objective.

This is **negative-space definition** of system behavior — describing
what is unacceptable rather than what is good. Same family as
Popper's falsifiability, type checkers (which reject ill-typed
programs but never construct correct ones), the Hippocratic *first do
no harm*, TDD red-green, apophatic theology. Underexplored design
space in the LLM/agent world.

### Component audit against this framing

Every Aegis gate is a rejection gate, not a goal gate:

| Component | Action | Negative-space form |
| :--- | :--- | :--- |
| Ring 0 (syntax / cycle) | parse / cycle check | rejects unparseable + cyclic — does not define "good code" |
| Ring 0.5 (signals) | measurement | observation only, never a verdict |
| PolicyEngine | rule table over signals | `fan_out > 20 → BLOCK`. Not "lower it to 5", "above 20 not accepted" |
| ToolCallValidator (T1+T2) | claim vs reality | "you claimed to write but didn't / wrote something else" → reject |
| IntentBypassDetector | semantic vs label | "you labelled teaching but produced code" → reject |
| **Cost-aware regression rollback** | `after_cost > before_cost → revert` | the centerpiece — entire judgement is "got worse, undo" |
| Validator | anchor / diff check | rejects unmatched anchors — does not specify how to write a good anchor |

The only positive direction in the system is the **user-supplied
task** that the planner consumes. Aegis itself defines no positive
target.

### Two important refinements

**1. Aegis does not guarantee improvement.**

Mathematically, monotone non-degradation under bounded action space
converges. Empirically, the V1 sweep shows the bound isn't free:
`gemma × regression_rollback` 5/5 never converged in 3 iterations
because the LLM kept producing degrading patches and could not find a
non-degrading path. Aegis was doing the right thing the entire time
(rejecting every regression), but the LLM's search space lacked a way
out.

So the precise claim is:

> Aegis ensures degradation is not silently accepted.
> Whether progress occurs depends on whether the LLM can find a
> non-degrading path within its budget.

This is why **stalemate detection** (V1.1 Finding C / Gap 1) is
necessary — when the LLM cannot find a non-degrading path, the
system must explicitly say "I see you are stuck" rather than silently
exhaust `max_iters`. **Stalemate is also a form of rejection** —
rejection of the pretense of progress.

**2. Aegis currently controls *code state*, not *task outcome*.**

A subtle but critical distinction:

- **What Aegis judges today:** Did this code-state transition make
  things worse? — using syntax / cycle / signals / cost.
- **What Aegis does *not* judge today:** Did the original task
  actually get done? — bug actually fixed, feature actually working,
  refactor actually achieved its design intent.

These are highly correlated but not the same. Concrete bug-fix
example:

| Aspect | Aegis verdict (today) | Reality |
| :--- | :--- | :--- |
| Code didn't get more complex | ✓ | — |
| No new circular dependency | ✓ | — |
| Cost didn't grow | ✓ | — |
| Bug actually fixed | (not checked) | might still be there |

So the V1 system is more precisely a **code-state safety harness**,
not a *task completion verifier*. This is intentional and *correct*
for V1: starting with "task understanding" pulls you into LLM-judge /
semantic-correctness territory, which is unstable, domain-dependent,
and ungeneralizable. V1 nails the floor (code state) before reaching
for the ceiling (task semantics).

### Where the layers fit this framing

| Layer | Scope | Relation to framing |
| :--- | :--- | :--- |
| **A** (per-gate trace event) | atomic decisions inside one LLM call | each gate is a rejection valve; verdict vocabulary is `PASS / BLOCK / WARN / OBSERVE` |
| **B** (per-iteration DecisionPattern) | one plan-validate-apply cycle | derives from "did the patch survive". Names like `REGRESSION_ROLLBACK`, `VALIDATION_VETO` describe **how the rejection fired**, not how the code improved |
| **C** (per-task TaskVerdict) | entire task lifecycle | **operational** as of V1.5 — `aegis/runtime/task_verifier.py` defines the `TaskVerifier` Protocol + 5 `TaskPattern`s (`SOLVED / INCOMPLETE / ABANDONED / NO_VERIFIER / VERIFIER_ERROR`); each scenario ships its own verifier in `tests/scenarios/<name>/verifier.py`. **Negative-space contract pinned by `tests/test_task_verifier.py`** — TaskVerdict has no `retry / feedback / hint / advice / guidance` field, TaskVerifier Protocol exposes only `.verify()`. Verdict cannot reach Layer B. |

**Layer A + B = code-state safety harness (V1).**
**Layer C = task outcome layer (V1.5 — built, evidence forward-only because `runs_pre_v1/` snapshots predate the verifier).**

### Why this framing differentiates Aegis from other tools

| Tool | Bet |
| :--- | :--- |
| Claude Code | I can make the LLM use tools well |
| Aider | I can make the LLM write code obediently |
| LangSmith | I can make you see what the LLM is doing |
| **Aegis** | **I ensure worse outcomes are not retained** |

This positioning is derived directly from the mechanism (cost-aware
regression rollback + invariant enforcement at module boundaries),
not retrofitted as marketing language. Anyone who asks "why?" gets
pointed at the code, not at a slogan deck.

### Design rule for all future work (Gap 1/2/3 and beyond)

Every design decision must pass this check:

> **Is this rejecting degradation, or directing toward a goal?**

If the latter, it is the wrong direction for Aegis. This rule applies
recursively:

- Gap 1 (stalemate detection) ✓ — rejects the pretense of progress
- Gap 2 (TaskVerifier) ⚠ — must stay in Layer C, must not feed back
  into Layer B (otherwise Aegis becomes a goal-seeker for "task
  satisfaction")
- Gap 3 (HITL) ✓ — humans become an additional rejection channel
  ("this verdict is unacceptable, revert"), not a teaching channel

---

## The five layers

| Layer | Question | What counts as evidence |
| :--- | :--- | :--- |
| **L1 — decision exists** | Does the loop emit anything machine-readable per iteration? | Non-empty `events[]` with `decision_pattern` per run |
| **L2 — decision boundary correct** | When the system says *regression*, did regression actually happen? | `regression_rollback` paths fire on a scenario designed to regress, **don't** fire on scenarios designed to improve |
| **L3 — decision influence** | Does the previous iteration change the next one? | Different `plan_id` + different patch shape iter-over-iter; multiple distinct trajectories on the same input |
| **L4 — decision stability** | Is the distribution of trajectories under repeated runs healthy (variable but not chaotic)? | Same (scenario, model) over N runs converges most of the time, with bounded-variability paths |
| **L5 — decision cross-model** | Does the system absorb model-family variance — same gates fire even when the LLM behaves differently? | Same scenario × different model produces different decision paths but the *gate vocabulary* (pattern enum) is unchanged |

---

## Sweep overview

Two sweeps documented here. The V1 sweep (2026-04-25) gathered Layer
A/B evidence with three model families. The V1.5 sweep (2026-04-26)
ran after Layer C (TaskVerifier) shipped, replacing two providers
with stronger / faster alternatives and producing the *first dataset
where task-level outcome (`SOLVED / INCOMPLETE / ABANDONED`) is
distinguished from pipeline outcome (`pipeline_success`)*.

| Sweep | Date | Runs | Wall-clock | Layer C? | Model matrix |
| :--- | :--- | :--- | :--- | :--- | :--- |
| V1 | 2026-04-25 | 60 (52 saved, 8 lost to ms-precision collision) | ~70 min | ❌ | gemma + gemini-2.5-flash + ling |
| **V1.5** | **2026-04-26** | **100 (all saved)** | **~83 min** | ✓ | **gemma + 3 Groq (llama-3.3-70b, gpt-oss-120b, qwen3-32b) + ling** |

V1.5 dropped `gemini-2.5-flash` (free-tier quota too small for
N=5 sweeps) and `z-ai/glm-4.5-air:free` (single calls observed
taking 36 minutes on OpenRouter's Z.AI free backend, would have
made the sweep run >7 hours). Added Meta Llama 3.3 70B, OpenAI
gpt-oss-120b, Alibaba Qwen3-32B via Groq for fast cross-family
coverage. Gemma kept as continuity baseline; ling kept as the
known-weak control case.

---

## V1.5 stability + Layer C summary

```
| scenario            | model                          | n | pipeline ok | task SOLVED                 | iter min/med/max | rollback | distinct paths | mode path                                                          |
|---------------------|--------------------------------|---|-------------|-----------------------------|------------------|----------|----------------|--------------------------------------------------------------------|
| syntax_fix          | gemma-4-31b-it                 | 5 | 5/5         | 5/5                         | 1/1/1            | 0        | 1              | applied_done (5/5)                                                 |
| syntax_fix          | llama-3.3-70b-versatile        | 5 | 5/5         | 5/5                         | 1/1/1            | 0        | 1              | applied_done (5/5)                                                 |
| syntax_fix          | openai/gpt-oss-120b            | 5 | 5/5         | 5/5                         | 1/1/2            | 0        | 2              | applied_done (3/5)                                                 |
| syntax_fix          | qwen/qwen3-32b                 | 5 | 2/5         | 2/5 (3 abandoned, TPM)      | 0/0/1            | 0        | 2              | (mixed)                                                            |
| syntax_fix          | inclusionai/ling-2.6-1t:free   | 5 | 4/5         | 5/5 (one SOLVED-despite!)   | 1/1/3            | 0        | 3              | applied_done (3/5)                                                 |
| fanout_reduce       | gemma-4-31b-it                 | 5 | 4/5         | 4/5 (1 abandoned)           | 0/1/1            | 0        | 2              | applied_done (4/5)                                                 |
| fanout_reduce       | llama-3.3-70b-versatile        | 5 | 0/5         | 0/5                         | 0/3/3            | 0        | 3              | silent_done_veto → silent_done_veto → validation_veto (2/5)        |
| fanout_reduce       | openai/gpt-oss-120b            | 5 | 0/5         | 0/5                         | 0/0/1            | 0        | 2              | (TPM-dominated)                                                    |
| fanout_reduce       | qwen/qwen3-32b                 | 5 | 0/5         | 0/5                         | 0/0/1            | 0        | 2              | (TPM-dominated)                                                    |
| fanout_reduce       | inclusionai/ling-2.6-1t:free   | 5 | 0/5         | 0/5                         | 3/3/3            | 0        | 2              | silent_done_veto → validation_veto → validation_veto (3/5)         |
| lod_refactor        | gemma-4-31b-it                 | 5 | 5/5         | 5/5                         | 1/2/3            | 0        | 4              | silent_done_veto → applied_done (2/5)                              |
| lod_refactor        | llama-3.3-70b-versatile        | 5 | 1/5         | 0/5 (★ 1 INCOMPLETE caught) | 1/2/3            | 0        | 5              | silent_done_veto → applied_continuing → silent_done_veto (1/5)     |
| lod_refactor        | openai/gpt-oss-120b            | 5 | 0/5         | 0/5                         | 0/0/3            | 0        | 2              | (TPM-dominated)                                                    |
| lod_refactor        | qwen/qwen3-32b                 | 5 | 0/5         | 0/5                         | 0/0/0            | 0        | 1              | (TPM-dominated)                                                    |
| lod_refactor        | inclusionai/ling-2.6-1t:free   | 5 | 1/5         | 0/5 (★ 1 INCOMPLETE caught) | 3/3/3            | 0        | 2              | silent_done_veto → validation_veto → validation_veto (4/5)         |
| regression_rollback | gemma-4-31b-it                 | 5 | 0/5         | 0/5                         | 3/3/3            | 3        | 3              | silent_done_veto → silent_done_veto → regression_rollback (2/5)    |
| regression_rollback | llama-3.3-70b-versatile        | 5 | 0/5         | 0/5                         | 0/3/3            | 3        | 5              | validation_veto → validation_veto → validation_veto (1/5)          |
| regression_rollback | openai/gpt-oss-120b            | 5 | 0/5         | 0/5                         | 0/0/3            | 2        | 3              | (mixed)                                                            |
| regression_rollback | qwen/qwen3-32b                 | 5 | 0/5         | 0/5                         | 0/0/0            | 0        | 1              | (TPM-dominated)                                                    |
| regression_rollback | inclusionai/ling-2.6-1t:free   | 5 | 0/5         | 0/5                         | 0/3/3            | 0        | 4              | silent_done_veto → validation_veto → validation_veto (2/5)         |
```

(Live aggregator output: `PYTHONPATH=. python scripts/v1_aggregate.py`.)

### What the ★ rows mean — Layer C in action

The two `★ INCOMPLETE caught` rows above are the empirical proof
that Layer C added signal beyond what `pipeline_success` could
detect:

- **`lod_refactor × llama-3.3-70b` run 3:** `pipeline_ok=True,
  iters=2, task_verdict=incomplete`, rationale `"orders.py
  max_chain_depth = 3 (> target 2)"`. The planner declared `done`
  at iter 2, the executor applied successfully, the loop returned
  `success=True`. **Without Layer C, this would be reported as
  llama-3.3-70b solving lod_refactor 1/5 times.** With the verifier
  attached, the actual `max_chain_depth` was measured against the
  task's structural target — and found wanting. True success rate:
  0/5.
- **`lod_refactor × ling` run K (similar):** identical shape — LLM
  said done, verifier disagreed, INCOMPLETE.

Conversely, the `syntax_fix × ling 4/5 pipeline_ok → 5/5 SOLVED`
cell shows the *opposite* direction — one run had `pipeline_ok=False`
(loop didn't reach `plan_done`) but the workspace `broken.py` parsed
fine when the verifier checked it. Layer C correctly upgraded the
verdict to SOLVED. **Pipeline success and task success are not
synonyms; the V1.5 sweep produced concrete examples of both
directions.**

---

## Layer-by-layer reading

### L1 — decision exists ✓

Every run with `iter > 0` produced an `events[]` array; every event
carries a `decision_pattern` (or fields enough to derive one). The
only empty-events runs are the gemini-2.5-flash quota-error rows,
where the planner failed before reaching the loop body. That is
*itself* a kind of decision (`error_at_provider`) but the V1 pattern
enum doesn't model pre-loop failures.

**Evidence:** all non-quota rows above have non-empty `pattern path`.

### L2 — decision boundary correct ✓

The `regression_rollback` scenario provokes refactors that
*increase* `max_chain_depth`, so a correct system must sometimes
detect regression and revert.

**V1.5 — rollback fire rate across capable models:**

| model | rollback runs |
| :--- | :--- |
| gemma-4-31b-it | 3/5 |
| llama-3.3-70b-versatile | 3/5 |
| openai/gpt-oss-120b | 2/5 |
| qwen3-32b | 0/5 (TPM-rate-limited before iter 0 in most runs) |
| ling-2.6-1t | 0/5 (validation_veto path before reaching apply) |

**Aggregate: 8/15 capable-model runs hit the rollback decision** —
proving the cost-aware comparator fires under real LLM behavior
*across multiple model families*, not just gemma.

The improving/trivial scenarios (`syntax_fix`, `fanout_reduce`,
`lod_refactor`) registered **zero rollbacks** across all 75 sweep
runs, as designed. The detector is correctly scoped: it fires when
work degrades structure, stays quiet when it doesn't.

**V1 baseline (preserved for comparison):** gemma alone, 2/5
rollback runs. V1.5 strengthens this to 8/15 across 3 capable
families.

**Caveat:** the firing rate is *the LLM's* hit rate at producing
cost-growing patches; not a guarantee the detector would catch
every regression. A synthetic detector test that *guarantees* a
regressing patch would isolate L2 better — V1.6+ follow-up.

### L3 — decision influence (qualified)

The system has a feedback channel from previous iterations into the
next plan: `ctx.previous_errors` and `ctx.previous_regression_detail`
both go into the planner prompt. Run-to-run variability is observable:

- **V1.5 — gemma × lod_refactor:** 5 runs, 4 distinct decision paths
  (`applied_done`, `silent_done_veto → applied_done`, two paths with 3
  iterations), iter counts ranging 1–3. **All 5 SOLVED**.
- **V1.5 — llama-3.3-70b × regression_rollback:** 5 runs, 5 distinct
  paths — the most diverse trajectory set in the sweep. Different
  patch shapes triggered different downstream fates (rollback,
  validation veto, applied-then-veto). The feedback channel is
  consumed; the LLM produces meaningfully different next-iter
  responses to its own past errors.
- **V1 — gemma × regression_rollback:** 5 runs, 4 distinct paths,
  one hit `regression_rollback` 3×, another hit it once — different
  patch shapes lead to different fates.

**What we claim:** previous-iteration data flows into the planner
prompt; the next iteration's `plan_id` and patch contents change as a
result. This is the *channel*, observable in the JSON snapshots.

**What we do *not* claim:** that the LLM is causally reasoning over
the feedback. We only see that the input changes and the output
changes; we have no instrumentation that distinguishes "LLM understood
the previous error" from "LLM produced a different sample because the
prompt is now different bytes." This distinction is honest, not
defeatist — the *system* is doing what we built it to do (channel
exists, gates fire); whether the LLM is *thinking* is a separate,
harder question.

### L4 — decision stability ✓ (variable but bounded)

The stability column above is the L4 result. Reading the V1.5
sweep:

- **Easy scenarios on capable models:** `syntax_fix` 5/5 SOLVED on
  4 of 5 model families (qwen lost to TPM), 1-iter `applied_done` —
  the floor case, deterministic-looking when task and model are
  both routine.
- **Harder scenarios on the same model** (`lod_refactor` × `gemma`):
  4 distinct paths over 5 runs, **all 5 SOLVED**. Healthy variance.
- **Hard-or-mismatched cells** (`regression_rollback` × any model,
  most cells × `ling-2.6`, all heavy-prompt cells × Groq's TPM-tight
  models): 0% task-SOLVED, but *decision paths are still bounded* —
  every observed path is one of the existing 7 named patterns, no
  `unknown` rows across 100 runs. **The system fails legibly.**

**No oscillation pathologies** observed (e.g. no
`regression_rollback → silent_done_veto → regression_rollback → ...`
patterns). Closest near-miss: V1's `gemma × regression_rollback`
run 3 hit `regression_rollback` three times in a row, and V1.5's
gemma × regression_rollback again hit 3 consecutive rollbacks in
one run. **Both would now be caught by Gap 1's
`THRASHING_DETECTED` (≥2 consecutive rollbacks)** — see
[V1.1 stalemate / thrashing detection][1] for that mechanism.

[1]: ./gap3_control_plane.md

### L5 — decision cross-model ✓ (V1.5 strengthened)

Five model families × four scenarios × 5 runs = 100 sweep runs.
The *gate vocabulary* was unchanged across all of them. Pattern
paths shift dramatically (capable Llama → fast `applied_done`;
ling → `validation_veto` chains; TPM-rate-limited Groq cells →
0-iter ABANDONED), but **the system never returned an `unknown`
pattern, never crashed, and always emitted a DecisionTrace +
TaskVerdict** across 100 runs.

The strongest L5 statement this data supports:

> **The mechanism is model-agnostic. Five model families
> (Google Gemma, Meta Llama 3, OpenAI gpt-oss, Alibaba Qwen,
> InclusionAI Ling) exercise the same finite gate vocabulary; the
> system surfaces — rather than papers over — model-specific
> weaknesses, and Layer C now catches the case where pipeline
> success and task success diverge.**

Specific V1.5 cross-model contrasts:

- **Layer C catches `INCOMPLETE` across 2 model families.** The
  `lod_refactor × llama-3.3-70b` and `lod_refactor × ling` cells
  each had 1 run where `pipeline_success=True` but the verifier
  measured `max_chain_depth > 2` and emitted INCOMPLETE. This is
  the *same failure mode* (LLM lying about completion) surfacing
  *across families* — the framework catches it the same way
  regardless of model.
- **`syntax_fix` cross-family floor:** gemma / llama-3.3-70b /
  gpt-oss-120b all 5/5 SOLVED. ling 5/5 SOLVED (one
  `pipeline_ok=False` upgraded by Layer C — workspace was actually
  fixed even though loop didn't reach `plan_done`). qwen 2/5 SOLVED
  (3 lost to TPM rate limits). **The mechanism's floor is uniform
  across capable models; degradation is purely a provider/model
  capability question, not a framework question.**
- **`fanout_reduce` cross-family ceiling:** only gemma converges
  (4/5). Llama / gpt-oss / qwen all 0/5, dominated by TPM
  rate-limit ABANDONED. Ling 0/5 from anchor format. **Heavy-prompt
  scenarios are the current capability ceiling on free-tier
  cross-family providers.**
- **`regression_rollback` rollback firing across families:** gemma
  3/5, llama-3.3-70b 3/5, gpt-oss-120b 2/5 — **8/15 capable-model
  runs hit the rollback decision**. The mechanism that V1 only
  proved with one model is now empirically validated across three.
- **ling-2.6 anchor mismatch persists from V1.** Same finding as V1:
  ling reasons correctly about the bug (`plan_goal` text is
  accurate) but its `context_before`/`context_after` strings don't
  match the matcher's exact-text contract. Open as a V1.6 question:
  Planner prompt constraint or matcher whitespace relaxation.
- **gemini-2.5-flash dropped from V1.5.** V1's gemini-flash data
  was data-limited by the free-tier 20-req/day cap. V1.5 replaced
  it with three Groq-served models for cleaner cross-family
  evidence; would re-add gemini-flash if a paid sweep is run.

---

## Findings during sweep (V1 + V1.5 backlog)

These were discovered *while gathering evidence*, not before, which
is itself an L4/L5 dividend — running the sweep surfaced things unit
tests can't.

### Resolved between V1 and V1.5

1. **Snapshot timestamp collision (V1).** ✓ Fixed in V1.5 — runner
   appends millisecond suffix. V1.5 sweep saved 100/100 snapshots.
2. **gemini-2.5-flash free tier insufficient (V1).** ✓ Resolved by
   substitution — V1.5 dropped gemini-flash and used three Groq
   models for cross-family evidence.
3. **`regression_rollback` × gemma never converges (V1 Finding C).**
   ✓ Surfaced as a stalemate / thrashing detection problem;
   resolved structurally in V1.1 (`STALEMATE_DETECTED` /
   `THRASHING_DETECTED` decision patterns), not by raising max_iters.
   See [Gap 3 control plane design][2] for how these patterns will
   route to escalation in V2.

[2]: ./gap3_control_plane.md

### Surfaced anew by V1.5

4. **`z-ai/glm-4.5-air:free` total-request latency.** OpenRouter's
   Z.AI free backend was observed producing valid responses that
   took **2,177 seconds to stream** in a single call. Aegis's
   urllib `timeout=120` parameter is per-socket-read, not total —
   it didn't fire because individual chunks arrived in time. Fix:
   `OpenAIProvider` gained `total_timeout=90` (worker thread +
   `Future.result(timeout=)` wraps the request; underlying urllib
   leaks but caller gets the wall-clock guarantee). GLM dropped
   from V1.5 default matrix.

5. **Groq free-tier TPM limits dominate multi-iter scenarios.**
   Models like `qwen/qwen3-32b` (6K TPM) ran 1 successful iter
   then 4 × 0.1s ABANDONED in `fanout_reduce` (one heavy prompt
   eats most of a minute's budget). `gpt-oss-120b` and
   `llama-3.3-70b-versatile` show the same pattern at slightly
   lower rates. Layer C correctly classifies these as ABANDONED
   with HTTP 429 rationale — the framework absorbs the constraint
   honestly, but operationally Groq's free tier isn't suitable for
   full N-runs sweeps on heavy-prompt scenarios.

6. **Layer C catches `INCOMPLETE` across model families** (★ feature
   of V1.5). `lod_refactor × llama-3.3-70b` and `lod_refactor × ling`
   each had 1 run with `pipeline_success=True` but verifier-failed
   → `task_verdict=incomplete`. Without Layer C, llama would have
   been counted 1/5 SOLVED on lod_refactor; the truth is 0/5. The
   pattern (LLM declaring done while structural goal unmet) is not
   model-specific, supporting the framework's family-independence
   claim.

7. **`ling-2.6` anchor format mismatch persists from V1.** Same
   finding, same shape. Planner prompt or matcher relaxation —
   open V1.6 question.

8. **No `noop_done` or `executor_failure` instances** in the V1.5
   sweep except in `lod_refactor` gemma (1 noop). Patterns reachable
   in code but rarely fire — V1.6 question whether they're real
   modes worth keeping or theoretical-only branches.

---

## V1.6 verification sweep — Gap 1 stalemate detection in real traffic

Run on 2026-04-26 immediately after V1.5 commit landed. Purpose:
verify that the new `STALEMATE_DETECTED` and `THRASHING_DETECTED`
patterns from V1.1 (`aegis/runtime/decision_pattern.py`) actually
fire under real LLM behavior — not just under synthetic unit tests.

The V1.5 sweep ran with pre-Gap-1 code (Python module cache
isolation), so this is the first dataset where Gap 1 detectors are
active.

### Sweep matrix

| scenario | model | runs | provider |
| :--- | :--- | :--- | :--- |
| regression_rollback | inclusionai/ling-2.6-1t:free | 5 | OpenRouter |
| regression_rollback | minimax/minimax-m2.5:free | 5 | OpenRouter |

Total: 10 runs. Both models on OpenRouter (operator chose: Gemma
too slow for verification, Groq budget too tight for full sweeps).

### Result — `STALEMATE_DETECTED` fires across two model families

**8 of 10 runs hit `STALEMATE_DETECTED` at iter 2** (the third
iteration), via the state-totals path: 2 prior iters of veto means
`signal_value_totals` never moved, so the third iter triggers the
detector and the loop terminates with the pattern recorded.

| model | iters | pattern path | task verdict |
| :--- | :--- | :--- | :--- |
| ling-2.6 | 3 | `silent_done_veto → validation_veto → STALEMATE_DETECTED` | abandoned |
| ling-2.6 | 3 | `silent_done_veto → silent_done_veto → STALEMATE_DETECTED` | abandoned |
| ling-2.6 | 3 | `silent_done_veto → silent_done_veto → STALEMATE_DETECTED` | abandoned |
| ling-2.6 | 3 | `validation_veto → validation_veto → STALEMATE_DETECTED` | abandoned |
| ling-2.6 | 3 | `validation_veto → validation_veto → STALEMATE_DETECTED` | abandoned |
| minimax | 3 | `silent_done_veto → validation_veto → STALEMATE_DETECTED` | abandoned |
| minimax | 3 | `silent_done_veto → validation_veto → STALEMATE_DETECTED` | abandoned |
| minimax | 3 | `silent_done_veto → validation_veto → STALEMATE_DETECTED` | abandoned |
| minimax | 1 | `silent_done_veto` | abandoned (provider timeout — total_timeout=90s caught) |
| minimax | 1 | `silent_done_veto` | abandoned (provider timeout — total_timeout=90s caught) |

The two non-`STALEMATE_DETECTED` runs both hit the
`OpenAIProvider.total_timeout=90s` wall-clock guard added in V1.5.
Layer C correctly classified them as ABANDONED with the timeout
message in the rationale — the framework absorbs provider failure
gracefully even when the planner doesn't make it past iter 1.

### What this validates

Compare with the V1 + V1.5 sweep paths for the same scenario+model
cell (pre-Gap-1):

```
V1   ling × regression_rollback: silent_done_veto → validation_veto → validation_veto
V1.5 ling × regression_rollback: silent_done_veto → silent_done_veto → validation_veto
V1.6 ling × regression_rollback: silent_done_veto → silent_done_veto → STALEMATE_DETECTED
                                                                       ^^^^^^^^^^^^^^^^^^
                                                                       new — Gap 1 detector firing
```

Same model, same scenario, same loop budget, three sweep
generations. Pre-Gap-1 the loop hit `max_iters` silently; post-Gap-1
the system explicitly *names* the failure mode and terminates with
the right reason in `pipeline_error`:

> `state stalemate — signal_value_totals unchanged for 3 iters; loop is making no progress`

This satisfies the "why did it stop?" check from the V1.1 design
discussion: the answer is no longer "max_iters" but
"STALEMATE_DETECTED".

### `THRASHING_DETECTED` — mechanism shipped, direct evidence pending

V1.6 did not produce a direct `THRASHING_DETECTED` trace because
ling and minimax both got stuck in veto chains before reaching
`Executor.apply()` — which is the precondition for
`REGRESSION_ROLLBACK` (and hence for ≥2 consecutive rollbacks =
thrashing).

**Indirect evidence is strong:** V1's gemma × regression_rollback
run 3 produced
`regression_rollback → regression_rollback → regression_rollback`
under pre-Gap-1 code. Under Gap 1, that exact path triggers
`THRASHING_DETECTED` at iter 1 (after the second consecutive
`REGRESSION_ROLLBACK`). The pre-condition empirically exists; the
detector is mechanically simpler than the state-stalemate detector
that V1.6 directly validated.

V1.7 backlog: re-sweep gemma × regression_rollback or use a Groq
capable model (gpt-oss-120b hit rollback 2/5 in V1.5) to capture a
direct `THRASHING_DETECTED` trace.

---

## V1 completion checklist

Per the user's plan, V1 is *complete* when these five conditions
hold. Status reflects V1 + V1.5 evidence above:

- [x] **L1 — Decision existence observed.** Every non-quota / non-rate-limit
  run emits events with named decision patterns. 100/100 V1.5 runs
  produced TaskVerdicts; no `unknown` patterns observed across the
  combined 152 runs (V1 + V1.5).
- [x] **L2 — Decision boundary validated.** `regression_rollback`
  pattern fires *only* on the regression-designed scenario; **8/15
  capable-model runs hit it** (gemma 3, llama-3.3-70b 3, gpt-oss-120b 2),
  zero false positives across 75 runs of improving/trivial scenarios.
- [x] **L3 — Decision influence observed (with honest framing).**
  Plan-id and patch-shape vary iter-over-iter; previous-iter data
  flows into prompt. **llama-3.3-70b × regression_rollback produced
  5 distinct trajectories across 5 runs** — strongest single-cell
  L3 evidence. LLM reasoning causality still not claimed.
- [x] **L4 — Decision stability characterized.** Stability table
  shows distribution per (scenario, model). No oscillations, no
  `unknown` patterns across 100 V1.5 runs. Hard cells fail
  legibly within the 7-pattern vocabulary. Two `regression_rollback
  → regression_rollback → regression_rollback` near-misses across
  V1+V1.5 — both would now be caught by Gap 1's THRASHING_DETECTED
  (V1.1 mechanism shipped, not yet evidence-validated by sweep).
- [x] **L5 — Cross-model consistency observed.** **Five model
  families** in V1.5 exercise the same gate vocabulary (Google
  Gemma, Meta Llama 3, OpenAI gpt-oss, Alibaba Qwen, InclusionAI
  Ling); zero crashes, zero `unknown`, zero pattern-vocabulary
  exceptions. INCOMPLETE pattern caught across 2 families —
  framework-independence empirically supported.

### V1.5 addition (Layer C)

- [x] **Layer C — Task-level outcome separated from pipeline-level
  outcome.** TaskVerdict pattern distinguishes SOLVED / INCOMPLETE
  / ABANDONED / NO_VERIFIER / VERIFIER_ERROR. Sweep produced 2
  empirical INCOMPLETE cases (LLM declared done, verifier disagreed)
  and 1 empirical "SOLVED-despite-incomplete-pipeline" case (loop
  didn't reach plan_done but workspace was actually fixed). **Both
  directions of the pipeline/task divergence appear in real
  traffic** — Layer C is not a theoretical refinement.

**V1 + V1.5 are complete.** The mechanism shipped; the claim now
has evidence across 5 model families with task-level verification.
Gap 1 (V1.1 stalemate / thrashing) shipped as mechanism, awaits
evidence in V1.6+ verification sweep. Gap 3 (control plane / HITL)
shipped as design doc, awaits implementation. Gaps surfaced during
sweeps are recorded above as future-work backlog, not deferred
unknowns.
