# Aegis Framework

This document defines Aegis at the framework level.

> **Status of this document.** Aegis as a five-layer admission
> control framework is articulated here for the first time. The
> reference implementation (`aegis-agent`) realises the framework
> on the code-change domain. Other domains named in §2 are framework
> targets, not validated cases. This document defines intent +
> abstraction; per-domain evidence accumulates separately.

`aegis-agent` is one implementation case: Aegis applied to
agent-driven code changes. The framework itself is broader. It is a
layered control architecture for deciding whether an agent-proposed
state transition may persist in an external system.

---

## 1. Aegis As A Layered Control Framework

Aegis models agent behavior as a state-transition problem rather
than a prompt-quality problem.

The core claim is simple:

> An agent may propose actions, but it does not directly alter
> external state. Every proposed transition must pass through a
> control stack before it is allowed to persist.

At the framework level, Aegis has five layers:

1. **Current State**
   The current observable state of the system. This is the baseline
   against which any proposed change is judged.
2. **Proposed Transition**
   The change the agent wants to make to the current state. Aegis
   evaluates the proposed transition, not the agent's internal
   reasoning or intent.
3. **Constraint Layer**
   The rules applied to the proposed transition. This layer contains
   both hard constraints and soft risk signals.
4. **Decision Layer**
   The control verdict produced from the constraint evaluation:
   `allow`, `warn`, `block`, or `escalate`.
5. **Commit / Rollback**
   Only approved transitions are allowed to persist. Rejected
   transitions do not commit; failed or invalid applied transitions
   must be rolled back or compensated for.

Three design principles follow from this shape:

- Aegis controls admission, not generation.
- Aegis evaluates transitions, not intentions.
- Human review is exceptional, not default.

`escalate` is therefore a special decision outcome, not the normal
operating mode. Humans appear when rules are insufficient, risk is
too high, or competing constraints cannot be safely resolved by the
system.

> **Reference implementation status.** `aegis-agent` currently
> realises `allow` / `warn` / `block` (`PolicyVerdict` in
> `crates/aegis-core/src/policy.rs`). `escalate` is part of the
> framework definition and lands in the reference impl when the
> first concrete escalation case emerges in the code domain — the
> trigger condition is pinned in the [PR #4
> review](https://github.com/wei9072/aegis/pull/4#issuecomment-4332452909).

---

## 2. Domain-Agnostic Core vs Domain-Specific Adapters

For Aegis to function as a framework rather than a code-only
product, it must separate what stays invariant across domains from
what changes with each domain.

### Domain-Agnostic Core

These parts define Aegis itself:

- **State-transition model**
  Agent actions are represented as proposals to change external
  state.
- **Layered control flow**
  Every proposal passes through:
  `state -> transition -> constraints -> decision -> commit/rollback`.
- **Decision vocabulary**
  The framework produces `allow`, `warn`, `block`, or `escalate`.
- **Persistence discipline**
  Transitions that fail control do not persist. Transitions that are
  applied but later found invalid must be reversible or otherwise
  compensatable.
- **Control trace**
  Every evaluation leaves a machine-readable record of what was
  proposed, which constraints fired, what decision was made, and
  whether the system committed or rolled back.

### Domain-Specific Adapters

These parts are supplied per domain:

- **State representation**
  What the system state looks like in that domain.
- **Transition representation**
  What form an agent proposal takes in that domain.
- **Constraint semantics**
  What counts as a hard violation, a soft risk increase, or an
  escalation condition.
- **Execution backend**
  Where approved transitions are actually committed.
- **Rollback semantics**
  How rejected or invalid transitions are reversed, compensated for,
  or quarantined.

This is the key boundary:

> Aegis defines the control architecture; adapters define the state
> semantics.

Under this view, `aegis-agent` is simply:

`Aegis Core + Code Adapter`

The same framework could be instantiated as:

- `Aegis Core + Operations Adapter`
- `Aegis Core + Finance Adapter`
- `Aegis Core + Healthcare Adapter`

The framework stays stable even as the state model, constraints, and
execution substrate change.

---

## 3. Why This Framing Fits Enterprise Agent Adoption

Enterprises usually do not fail to adopt agents because the models
are too weak in the abstract. They fail because the organization
cannot safely allow an agent to change real systems without
predictable control over what may persist.

That makes state-transition control a better primary abstraction
than "making the agent smarter."

This framing is enterprise-suitable for three reasons:

- **It is system-facing rather than model-facing.**
  Enterprises govern what enters production systems, not what the
  model "meant." Aegis evaluates externalized transitions.
- **It supports partial automation.**
  Safe transitions can pass automatically; uncertain transitions can
  warn or escalate. This allows agent-native operation without
  making human review the default bottleneck.
- **It maps cleanly to accountability.**
  A machine-readable control trace makes it possible to inspect what
  was proposed, why it was allowed or blocked, and where human
  override entered the process.

The practical consequence is that Aegis is not primarily a tool for
improving model behavior. It is a framework for controlling how
agent behavior is admitted into real-world state.

That is why `aegis-agent` should be understood as a reference
implementation, not the full definition of Aegis. The broader value
lies in the control framework itself.
