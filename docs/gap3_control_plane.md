# Gap 3 — Control Plane (design, not implementation)

> **Code blocks below are Python-syntax for readability — the
> actual implementation will land in Rust (`crates/aegis-runtime/`
> or a new `crates/aegis-control/`). The trait shape, the
> `should_stop` / `StopVerdict` / `HumanVerdict` semantics, and
> the Critical Principle are language-agnostic and survive the
> Rust port unchanged.**

This document defines the *interface* between the pipeline (which
detects) and the controller (which decides what to do about
detection). It does not specify implementation. Code lands after
the V1.5 sweep produces evidence about *which* detection points
humans actually need to be in the loop for.

The doc was written when Gap 1 (stalemate / thrashing detection)
deliberately built detection *and* termination into the loop
together — the V0.x `pipeline._run_loop` mixed observation with
control via `_step → terminate_reason`. The Rust port preserved
that coupling in `aegis-runtime::loop_step::step_decision` for
parity. Gap 3 is the structural fix: the controller becomes
caller-injected, not loop-internal. Writing code before nailing
the interface risks producing a first-pass HITL that needs to be
torn out for the same reason.

[1]: ./v1_validation.md#layer-by-layer-reading

---

## Hard rule before any specifics

This document, and any code that lands from it, must pass the
[negative-space check](../README.md):

> **Is this rejecting degradation, or directing toward a goal?**

The control plane sits dangerously close to "directing toward a
goal" because it is the layer where retries and escalations live. The
section [Critical principle](#critical-principle) below is the
non-negotiable hill. Re-read it before any change.

---

## 1. Control boundary

Two layers, no leakage between them.

**Pipeline (existing).** Owns the iteration loop. Consumes a task,
calls planner / validator / executor, emits one `IterationEvent` per
iteration carrying observations (signals, decision pattern, stalemate
/ thrashing flags). Knows nothing about humans, escalation, or
retries. Stops only when one of:

- Planner declares `done` with a successful apply
- `max_iters` exhausted
- The injected `should_stop` callback returns a stop verdict (Gap 3
  introduction; see §2)

**Controller (Gap 3, new).** Sits *outside* `pipeline.run()`. Reads
each `IterationEvent` via the existing `on_iteration` callback and/or
the post-run `PipelineResult`. Decides whether to terminate the
current run, escalate to a human, or let the loop continue. Owns the
human-loop contract.

```
┌────────────────────────────────────────────┐
│           Controller (Gap 3)               │
│  reads events, decides stop/escalate       │
│  owns human loop                           │
└──────────┬─────────────────────────────────┘
           │ should_stop(event)
           │ + on_iteration(event)
┌──────────▼─────────────────────────────────┐
│        Pipeline (existing)                 │
│  loop, planner, validator, executor        │
│  emits IterationEvent                      │
│  knows nothing about humans                │
└────────────────────────────────────────────┘
```

**Layer-crossing rules** (these break the boundary):

- ❌ Pipeline reading from controller state mid-iteration
- ❌ Controller injecting hints into the planner prompt
- ❌ Controller writing to `IterationEvent` (events are pipeline
  output, not controller input)
- ❌ Pipeline knowing whether a human is involved
- ✅ Controller reads events, returns boolean-ish verdicts that
  pipeline can act on (terminate the loop, that's it)

---

## 2. `should_stop` interface

Replaces the current `_step → terminate_reason` private machinery
with a public, caller-injected hook.

```python
@dataclass(frozen=True)
class StopVerdict:
    """What the controller decides about the running pipeline.

    `stop=False` means the controller has no opinion this iter; the
    loop continues. `stop=True` means the loop returns immediately,
    and `next_action` tells the orchestration layer what to do
    about it.
    """
    stop: bool
    reason: str = ""
    next_action: NextAction = NextAction.TERMINATE


class NextAction(str, Enum):
    TERMINATE = "terminate"   # done, no recovery
    ESCALATE  = "escalate"    # hand to human, await verdict
    PAUSE     = "pause"       # hand to human, with retry timeout
```

Pipeline signature gains:

```python
def run(
    task, root, provider,
    ...,
    on_iteration: IterationCallback | None = None,
    verifier: TaskVerifier | None = None,
    should_stop: Callable[[IterationEvent], StopVerdict] | None = None,
) -> PipelineResult:
```

Default `should_stop=None` → loop runs to natural completion
(planner done OR max_iters), preserving backward compat.

**Stop ≠ terminate.** Critical distinction enforced by the verdict
shape: `stop=True, next_action=ESCALATE` halts the loop *but does
not declare the task failed*. The orchestration layer is responsible
for routing to a human and resuming work (or not) based on the
human's verdict.

`PipelineResult` gains:

```python
@dataclass
class PipelineResult:
    ...  # existing fields
    stop_verdict: StopVerdict | None = None  # set if loop stopped via should_stop
```

`task_verdict` (Layer C / Gap 2) and `stop_verdict` (Gap 3) are
*independent* fields. They can coexist (e.g. controller escalated
mid-loop AND the verifier later said the workspace is incomplete).

---

## 3. Escalation model — default policy

Ships as a separate `aegis/control/default_policy.py`. Callers can
substitute their own.

The default policy is intentionally sparse — every escalation
trigger should be one a human actually needs to see, not "interesting
to log". Logging is what the trace is for.

| Trigger | `next_action` | Rationale |
| :--- | :--- | :--- |
| `THRASHING_DETECTED` | `ESCALATE` immediately | System has emitted ≥2 consecutive `REGRESSION_ROLLBACK`. The decision system is firing correctly; the LLM cannot find a non-degrading path. This is exactly the situation where another human's structural intuition might break the deadlock. |
| `STALEMATE_DETECTED` (state-totals) | `ESCALATE` after second occurrence in same task | Single stalemate can be a transient — the next plan might break it. Two stalemates means the loop has now exhausted its own moves twice. Worth interrupting. |
| `STALEMATE_DETECTED` (plan-repeat + no movement) | `ESCALATE` immediately | Stronger combined signal (see [V1.1 stalemate triggers][2]). One occurrence is enough. |
| `EXECUTOR_FAILURE` | `TERMINATE` | Tooling bug, not a decision-system question. Don't burn human attention. Surface in trace and let ops investigate. |
| Pipeline error before iter 0 (provider 5xx, quota, etc.) | `TERMINATE` | Same — infrastructure, not decision. |
| All other patterns | `CONTINUE` | Default: trust the loop. |

Layer C task-level escalation is computed *after* `pipeline.run()`
returns, separately from `should_stop`. See §5.

[2]: ./v1_validation.md#layer-by-layer-reading

---

## 4. Human loop contract

When `next_action == ESCALATE`, the orchestration layer routes the
situation to a human and awaits a `HumanVerdict`. The verdict shape
is the spec for what humans can decide; the routing mechanism (CLI
prompt, Slack message, web UI) is out of scope here.

```python
class HumanVerdict(str, Enum):
    APPROVE = "approve"   # human inspected workspace, declares it good enough
    RETRY   = "retry"     # human starts a new pipeline.run() (possibly with edits)
    ABORT   = "abort"     # human declares this task unsolvable here
```

`HumanVerdict` semantics in detail:

**APPROVE.** Human inspected the workspace and decided the task is
satisfied even though the verifier said otherwise (or even though
no verifier ran). The TaskVerdict is overridden to a new
`HUMAN_APPROVED` pattern. **Human is the verifier of last resort.**
This does not retroactively change the IterationEvents; the trace
remains accurate (system thought task wasn't done; human disagreed).

**RETRY.** Human chooses to invoke a new `pipeline.run()`. The
human may edit the task description, the scope, or other inputs
before retrying. From the system's perspective, this is just a new
task: previous run's events are archived for context but **do not
flow into the new run's planner prompt**. If the human wants to
provide a hint, they edit the task description — they are the
controller, not the planner's tutor.

**ABORT.** Human declares this task unsolvable in the current
configuration. TaskVerdict gains a new pattern `HUMAN_ABORTED`.
Distinct from `ABANDONED` (which is the system giving up); ABORTED
is the human giving up.

---

## 5. Relationship to `TaskVerdict` (Layer C)

`TaskVerdict.pattern` gains three values to cover human-side
outcomes. Existing patterns (`SOLVED`, `INCOMPLETE`, `ABANDONED`,
`NO_VERIFIER`, `VERIFIER_ERROR`) stay untouched.

| New TaskPattern | Triggered by | Semantics |
| :--- | :--- | :--- |
| `HUMAN_APPROVED` | HumanVerdict.APPROVE | Human declared the task complete; overrides verifier (or fills in for missing verifier) |
| `HUMAN_ABORTED` | HumanVerdict.ABORT | Human declared the task unsolvable; distinct from system-side `ABANDONED` |
| `ESCALATED_PENDING` | `should_stop` returned `ESCALATE` and no human verdict has come back yet | Long-lived state; the controller is waiting on a human |

**Composition rule** (the V2 invariant):

```
final_task_verdict = combine(
    pipeline_outcome,    # whether the loop reached plan_done
    verifier_result,     # what the structural verifier said (Gap 2)
    control_decision,    # what the controller / human said (Gap 3)
)
```

`combine()` lives in the controller, not the pipeline. Pipeline
remains pure — no awareness of human or controller existing.

**Hard rule:** `control_decision` only writes to TaskVerdict. It
**never** writes back to IterationEvent and **never** influences a
later iteration in the same pipeline.run() call. If a human says
RETRY, that produces a *new* `pipeline.run()`, not a modified
continuation of the previous one. This is the same isolation that
keeps the verifier in Layer C — the same structural reason applies
here.

---

## Critical principle

**The control layer must not become a retry engine.**

This is the hill. If we ship code that does any of the following,
the framing has been violated:

- ❌ `if verifier_failed: pipeline.run(...)` — automated retry
- ❌ `if stalemate_detected: hint = generate_hint(...); pipeline.run(task + hint)` — system-generated coaching
- ❌ `for attempt in range(N): pipeline.run(...)` — retry-until-success loop in the controller
- ❌ Any control logic that consumes `verifier_result.evidence` and decides "well, fan_out is high, let me try again with a different prompt"

The control layer's job is to **route** signals (to humans, to logs,
to TaskVerdict). Any actuation (retrying, hinting, re-planning) must
be a *human* decision, not a *system* decision. The system can
present options to the human; only the human can pull the trigger.

The reason this matters is the same reason Gap 2 keeps the verifier
in Layer C: the moment the system starts optimizing for an external
goal (passing the verifier, getting human approval), it stops being
a decision system that judges its own work and becomes an optimizer
that bends its own invariants in pursuit of success. Aegis exists
specifically to *not* do that.

If a future PR proposes "let's just have the controller auto-retry
on STALEMATE with a slightly modified prompt" — that PR is a
framing-level conversation, not a code-review conversation.

---

## What V1.5 sweep evidence will inform

These decisions in this design are *defaults*, not laws. The V1.5
sweep (in progress as of writing) will give us empirical data on:

- **Which patterns actually fire in real runs.** If `THRASHING_DETECTED`
  never fires across 100 sweep runs, the "escalate immediately on
  thrashing" default is moot — adjust the trigger or remove from
  the default policy.
- **Which scenarios produce STALEMATE vs ABANDONED most often.** If
  ABANDONED dominates and STALEMATE never fires, the detection
  thresholds need tuning before escalation policy matters.
- **The shape of LLM behavior across model families.** A human-loop
  policy that makes sense for Gemma might be wrong for Ling-2.6.
  Cross-model variance might justify per-model default policies.

After the sweep, this document gets a "validated against evidence"
section before any implementation work begins.

---

## Implementation order (when code finally lands)

1. **V1.5 sweep completes** — read [docs/v1_validation.md][3] for
   actual data on stalemate / thrashing fire rates per model.
2. **Update §3 (escalation policy) with evidence.** Triggers that
   never fire in real traffic don't need default escalation rules.
3. **Implement `StopVerdict` + `NextAction` enums** in
   `crates/aegis-runtime/src/control.rs` (or a new `aegis-control`
   crate). Pure data, no logic.
4. **Add `should_stop=` parameter to `pipeline.run()`.** Default
   `None`. Refactor `_step` so its `terminate_reason` becomes
   a derived `StopVerdict` produced *only* when `should_stop` is
   `None` (preserves V1.1 default behavior; new opt-in path uses
   the injected callback).
5. **Implement `aegis/control/default_policy.py`** matching §3.
   Unit tests pin the trigger conditions.
6. **Define `HumanVerdict` + `HUMAN_*` TaskPattern values.** Pure
   data first.
7. **Build a CLI human-loop adapter.** Simplest possible: when
   `next_action == ESCALATE`, prompt the operator at the terminal
   for `approve / retry / abort`. Slack / web UI later.
8. **Compose into a top-level orchestrator.** Reads scenario, runs
   pipeline with `should_stop=default_policy`, on stop routes to
   human adapter, on human verdict either records APPROVE/ABORT
   in TaskVerdict or kicks off a new pipeline.run() for RETRY.

Steps 3–6 can land in one PR (interface only, no behavior change).
Steps 7–8 are the actual HITL milestone.

---

## Out of scope for Gap 3

Recorded so they don't accidentally creep in:

- **Async / persistent escalation.** Default human loop is
  synchronous (controller blocks waiting for verdict). A queue-based
  escalation model where humans pick from a backlog is a separate
  Gap (4? 5?).
- **Multi-human approval.** Current spec assumes one human per
  escalation. Quorum / two-person review is future work.
- **Adaptive policy.** ROADMAP §4.3 (trust score / cross-layer
  reasoning) is what eventually replaces hand-coded default policy
  with learned policy. Out of scope until V2 has data.
- **Bypass / override mechanisms.** "Always escalate to me" or
  "never escalate" toggles are operational concerns, not part of the
  control-layer spec.
