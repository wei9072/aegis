# Future abstraction — Aegis as a domain-independent decision system

This document is **prior art for a future extraction**, not a
present-day commitment. Aegis V1.5 ships a working code-generation
decision system. The framework underneath that system is
domain-independent enough to be extracted — but extracting it now
would be premature optimization. This file records what the split
looks like so that **when** extraction happens, the work is mostly
mechanical renaming rather than re-derivation.

If you are reading this and tempted to start the refactor, read
[§ When this document becomes actionable](#when-this-document-becomes-actionable)
first.

---

## The split — framework vs plugin

Aegis V1.5 already separates two layers, even if the directory tree
doesn't make it obvious:

### Framework layer (domain-independent)

| Module | Responsibility | Why it's framework |
| :--- | :--- | :--- |
| `aegis/runtime/trace.py` — `DecisionTrace`, `DecisionEvent`, 4 verbs (`PASS`/`BLOCK`/`WARN`/`OBSERVE`) | Records every gate's verdict | The vocabulary doesn't care what the gate is judging |
| `aegis/runtime/decision_pattern.py` — `DecisionPattern` enum (9 values), `derive_pattern()` | Names per-iteration shapes | `APPLIED_DONE` / `REGRESSION_ROLLBACK` / `STALEMATE_DETECTED` are abstract loop-state concepts, not code-specific |
| `aegis/runtime/task_verifier.py` — `TaskPattern` enum (5 values), `TaskVerdict`, `TaskVerifier` Protocol | Names task-level outcomes | `SOLVED` / `INCOMPLETE` / `ABANDONED` are task concepts, not code concepts |
| `aegis/runtime/pipeline.py` — `IterationEvent`, `PipelineResult`, `_run_loop`, `_step`, `_is_state_stalemate`, `_is_thrashing` | Loop control + sequence-level meta-decisions | Plan→Validate→Execute→Re-analyze is a generic decision-system pattern; stalemate / thrashing detectors operate on history lists alone |
| Layer A/B/C isolation rules + negative-space framing | Architectural contract | Reread the framework note in `aegis_core_framing_negative_space` memory — these are philosophy, not implementation |

### Plugin layer (code-generation specific)

| Module | What's domain-specific |
| :--- | :--- |
| `aegis/analysis/signals.py` + Rust `aegis-core-rs` | "Structural signals" (fan_out / max_chain_depth / cycle) only make sense for Python code |
| `aegis/ir/patch.py`, `aegis/runtime/validator.py`, `aegis/runtime/executor.py` | `PatchPlan` data model + anchor matching + diff application + filesystem rollback |
| `aegis/agents/planner.py`, `aegis/agents/llm_adapter.py`, providers under `aegis/agents/` | LLM as planner; serializing plans to/from LLM completions |
| `aegis/policy/engine.py`, `aegis/intent/`, `aegis/toolcall/`, `aegis/delivery/`, `aegis/enforcement/` | Code-quality–specific gates (Ring 0 syntax, fan_out policy, intent classifier on natural-language prompts, etc.) |
| `aegis/runtime/pipeline.py::_regressed`, `_total_cost`, `_regression_detail` | Currently sums `Signal` values — the cost function is plugged in via Signal type |
| Per-scenario verifiers under `tests/scenarios/*/verifier.py` | Each verifier knows what "task done" means for its specific scenario |

---

## What the abstracted framework would look like

The shape is essentially what `pipeline.run()` already does, with
five injected dependencies instead of hard-coded ones:

```python
# Hypothetical aegis-core API — DO NOT IMPLEMENT YET.

State = TypeVar("State")  # whatever the world looks like (codebase, schema, portfolio, ...)
Plan  = TypeVar("Plan")   # whatever the planner outputs (PatchPlan, SQL diff, OrderList, ...)


class Planner(Protocol[State, Plan]):
    def plan(self, ctx: Context[State]) -> Plan: ...


class Validator(Protocol[Plan]):
    def validate(self, plan: Plan) -> list[ValidationError]: ...


class Executor(Protocol[State, Plan]):
    def apply(self, plan: Plan) -> ExecutionResult: ...
    def rollback(self, result: ExecutionResult) -> None: ...


class CostFunction(Protocol[State]):
    """Maps a state to a numeric cost. Aegis-codegen plugs in the
    sum-of-signal-values function; database-migration plugin would
    plug in query-plan cost; trading would plug in VaR; etc.

    `decision-system` is built around 'after_cost > before_cost ⇒
    rollback'. The function shape is what stays invariant; the
    measurement is what each domain provides.
    """
    def cost(self, state: State) -> float: ...
    def cost_breakdown(self, state: State) -> dict[str, float]: ...  # for regression_detail


class TaskVerifier(Protocol[State]):
    """Layer C — already correctly shaped today. No changes needed."""
    def verify(self, state: State, trace: list[IterationEvent]) -> VerifierResult: ...


class DecisionSystem(Generic[State, Plan]):
    def __init__(
        self,
        planner: Planner[State, Plan],
        validator: Validator[Plan],
        executor: Executor[State, Plan],
        cost: CostFunction[State],
        verifier: TaskVerifier[State] | None = None,
    ): ...

    def run(
        self,
        task: Task,
        initial_state: State,
        max_iters: int = 3,
        on_iteration: IterationCallback | None = None,
        should_stop: Callable[[IterationEvent], StopVerdict] | None = None,
    ) -> PipelineResult[State, Plan]: ...
```

`PipelineResult`, `IterationEvent`, `DecisionPattern`, `TaskVerdict`,
`StopVerdict` (Gap 3) all stay as-is — they're already
domain-independent.

The current `aegis/runtime/pipeline.py::_run_loop` is roughly 90% of
this implementation already. Extraction would be: pull out the loop,
parameterise the five protocols, move what stays into a new
`aegis-core` package, leave the rest in `aegis-codegen` as the first
plugin.

---

## Candidate domains

Best-fit shape: any **propose → validate → apply → measure cost →
optionally rollback** loop where:

- The state is observable and snapshot-able
- Plans are structured enough to validate before applying
- Cost is numerically comparable
- Rollback is meaningful (not strictly free, but possible)
- "Did the task get done?" is verifiable independently of the loop

| Domain | State | Plan | Cost | Why it fits |
| :--- | :--- | :--- | :--- | :--- |
| **Database migration** | schema + row counts + perf stats | DDL/DML diff | query-plan cost / lock-time / rollback-safety score | Plan→validate→apply→observe is textbook; rollback semantics are first-class. `STALEMATE_DETECTED` would catch "this migration keeps failing the same way" |
| **CI/CD canary deployment** | running version + production metrics | deployment manifest | `error_rate × latency_p99` | Canary→observe→rollback is exactly this loop. `THRASHING_DETECTED` catches deploy-rollback ping-pong |
| **SRE config rollout** | live config + alert noise | config diff | alert/incident rate | Same as CI/CD; smaller blast radius |
| **Trading risk system** | portfolio state | proposed order set | VaR / drawdown / margin usage | "Reject worse" *is* risk management. `REGRESSION_ROLLBACK` = automatic position unwind on limit breach |
| **RL policy iteration** | environment state | action sequence | -reward / regret | `STALEMATE_DETECTED` is exactly the "policy stuck in local optimum" detector hand-rolled by every RL practitioner |
| **Robot motion planning** | configuration space + obstacle map | trajectory | collision-distance / energy / time | Plan-validate-execute-safety-check is standard motion-planning shape |
| **Scientific experiment / hyperparameter search** | hypothesis + data | next experiment config | -likelihood / val loss | `THRASHING_DETECTED` = "hypothesis keeps getting refuted" |

The framework is most valuable where the domain has *both* of:
- A meaningful cost function (so regression-rollback isn't trivial)
- Real LLM / heuristic / human noise in the planner (so stalemate /
  thrashing actually fire — a perfectly deterministic planner never
  needs them)

---

## What does NOT fit

Honest exclusions, recorded so we don't accidentally pitch the
framework into wrong-shaped problems:

1. **Irreversible systems.** A robot arm that fired its launcher
   can't un-fire it. Cost-aware regression rollback is meaningless if
   `executor.rollback()` is impossible. The framework still works for
   the *planning* phase, but loses its core invariant once the
   executor commits.
2. **Free-text-only plans.** A diplomatic communiqué doesn't have a
   validator. The "plan" is unstructured natural language; nothing to
   `validate()` against. Aegis-style validation gates need plans
   that have shape.
3. **Continuous control loops.** Aircraft autopilot is a control loop
   but not a discrete plan-validate-apply iteration. The
   `IterationEvent` granularity doesn't fit; control-systems theory
   has its own established formalisms (PID, MPC, etc.).
4. **Pure monitoring / observation systems.** If you're not making
   decisions, you don't need a decision framework. You need a metrics
   system.
5. **Multi-agent / distributed decisions.** Aegis assumes one
   decision-maker per iteration. Coordinating decisions across
   distributed agents is a different problem class (consensus
   protocols, etc.).

---

## Preserve-the-shape actions to take in current code

These are small now, painful later. None of them block V2; treat as
"do opportunistically when touching the relevant file."

| Action | Why | Status |
| :--- | :--- | :--- |
| Keep `aegis/runtime/decision_pattern.py` and `aegis/runtime/task_verifier.py` free of `Signal` / `PatchPlan` imports | These are the cleanest framework candidates today; let them stay clean | ✓ already clean |
| `IterationEvent.signal_value_totals` will rename to `state_cost_totals` at extraction. Either dual-name now or open backlog. | Most leaky abstraction in the framework module today. The word "signal" is plugin-specific | ⚠ open — defer until extraction |
| Gap 3's `should_stop(event) -> StopVerdict` API | Already framework-shaped (operates on `IterationEvent`, returns abstract verdict). No domain leakage. | ✓ design pinned in `gap3_control_plane.md` |
| Continue distinguishing `aegis/runtime/` (framework candidate) from `aegis/agents/` + `aegis/tools/` + `aegis/analysis/` + `aegis/policy/` + `aegis/intent/` + `aegis/toolcall/` + `aegis/delivery/` + `aegis/enforcement/` (plugin) | The directory split already telegraphs the future package split | ✓ ongoing |
| Avoid adding new `Signal` imports to anything in `aegis/runtime/` | Same as row 1, applied prospectively | guideline |

---

## When this document becomes actionable

Three triggers, **all** must hit before extraction work starts:

1. **Aegis V2 is feature-complete.** Gap 3 (HITL control plane)
   implemented per `gap3_control_plane.md`. Adaptive Policy
   (ROADMAP §4.3) at least prototyped. Without these, extracting
   would freeze the framework in a less-mature state than it could
   be.
2. **At least one second-domain pilot exists.** Could be:
   - A side project where you actually try Aegis on database
     migrations / config rollouts / etc.
   - An external user proposing to use Aegis for non-code tasks.
   - A research collaboration that needs the loop semantics.

   Without a second concrete user, the abstraction risks codifying
   accidental code-gen specifics. "Two users make an interface; three
   make a framework."
3. **The second-domain pilot exposes ≥1 friction point.** The
   friction tells us *which* abstractions are wrong — e.g. trading
   might want a `THROTTLED` pattern; database might want a
   `PARTIAL_ROLLBACK` pattern. Without the friction, we'd ship the
   wrong enum.

If trigger 1 is met but 2 + 3 are not, **stop**. Document that the
shape is ready, do not extract speculatively.

---

## Naming for the eventual extraction

If/when extraction happens:

- `aegis-core` — the framework (currently `aegis/runtime/` minus
  `executor.py` minus `validator.py` minus the regression helpers
  that import `Signal`)
- `aegis-codegen` — current Aegis as the first plugin (everything
  else)
- Subsequent plugins: `aegis-dbmigrate`, `aegis-canary`, etc.

Repository topology probably moves to a monorepo with `packages/`
subdirs, or three separate repos with `aegis-core` as a Python
package dep.

---

## Why this isn't `docs/aegis_as_framework.md`

Naming intentionally cautious: "future abstraction" frames it as a
*possibility recorded for clarity*, not a *plan being executed*. If
this file gets renamed `aegis_framework_design.md` without the three
triggers above being met, that itself is a smell — push back.
