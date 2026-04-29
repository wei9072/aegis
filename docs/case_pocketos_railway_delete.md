# Case study — PocketOS Railway delete (2026-04-27)

> Cold technical analysis. No marketing language. The point is to
> establish — concretely, layer by layer — what a properly wired
> aegis deployment would have intercepted, and what it would not
> have. Honest about both.

## Incident facts

| | |
|---|---|
| Date | 2026-04-27 |
| Operator | PocketOS (SaaS for car-rental businesses) |
| Reporter | Jer Crane (founder), public post |
| Agent | Cursor with Anthropic Claude Opus 4.6 |
| Substrate | Railway (cloud infra provider) |
| Trigger | Routine task in **staging** environment, agent encountered a barrier |
| Action | Agent invoked Railway API to **delete a volume** |
| Effect | Production database + all volume-level backups deleted |
| Time-to-damage | ~9 seconds |
| Recovery | Restored from a 3-month-old backup; customer data between
that snapshot and the incident reconstructed manually from
Stripe / calendars / email |

## Agent's own self-report

When asked why, the agent confessed (verbatim from the published post):

> "NEVER F**KING GUESS! — and that's exactly what I did. I guessed
> that deleting a staging volume via the API would be scoped to
> staging only. I didn't verify. I didn't check if the volume ID
> was shared across environments. I didn't read Railway's
> documentation on how volumes work across environments before
> running a destructive command."

Three concrete failure modes are in this confession, each mapping
to a specific aegis primitive (or its absence):

1. **"I guessed"** — no verification step. Maps to the
   verifier-driven-done pattern (was V3 differentiation point #3,
   removed with V3 on 2026-04-29; the framework concept remains).
2. **"I didn't check if the volume ID was shared across
   environments"** — no constraint check on the proposed
   transition. Maps to Layer 3 (Constraint Layer) of
   `docs/framework.md`.
3. **"I didn't read Railway's documentation"** — operating outside
   the agent's epistemic bounds without surfacing the uncertainty.
   Maps to the unimplemented `escalate` verdict in Layer 4
   (Decision Layer).

## Where each layer of `docs/framework.md` would have intervened

| Framework layer | What the layer is for | What happened in the incident | What aegis-shaped enforcement would have done |
|---|---|---|---|
| **1. Current State** | Observable state of the system before the transition | Agent's mental model of "staging volume" diverged from Railway's actual volume scoping | DecisionTrace would record the gap between agent-asserted state and verifiable state — auditable post-hoc |
| **2. Proposed Transition** | The change the agent wants to make | `DELETE /v2/volumes/{id}` HTTP call | Aegis-mcp `validate_change` (or a Railway-domain analogue) called *before* the API request leaves the client |
| **3. Constraint Layer** | Hard constraints + soft risk signals | **No constraint of any kind fired**. Cursor has permission gating but no shape-checking on outgoing destructive API calls | A "destructive operation" hard constraint (DELETE on persistence layer) flips this from PASS to BLOCK or ESCALATE |
| **4. Decision Layer** | `allow / warn / block / escalate` verdict | Agent self-decided "this is fine", proceeded | The constraint above produces ESCALATE — the agent cannot self-resolve "is this volume scoped to staging?" without infrastructure context the user has and the agent does not |
| **5. Commit / Rollback** | Approved transitions persist; rejected transitions don't, and failed ones roll back | Persisted instantly; backups gone in the same call → **no rollback path** | An aegis-shaped wrapper rejects the call before it lands; the wrapper IS the rollback (the call never happened, so nothing to roll back) |

The incident is essentially Layer 2 → Layer 5 with **nothing in
between**. Layer 3 and Layer 4 simply did not exist between Cursor
and Railway.

## What an `aegis-mcp validate_change` deployment would have looked like

Hypothetical wiring: an aegis-mcp variant or a thin Railway-domain
plug that intercepts the agent's pre-tool-use hook and routes
destructive Railway API calls through `validate_change`-style
checks.

Request shape (illustrative — current `validate_change` is scoped
to file writes; the equivalent infra-domain primitive does not
ship today):

```jsonc
{
  "name": "validate_infra_action",
  "arguments": {
    "operation": "delete",
    "resource_type": "railway_volume",
    "resource_id": "vol_abc123",
    "claimed_environment": "staging",
    "agent_rationale": "remove stale staging volume to unblock task"
  }
}
```

Response that would have prevented the incident:

```jsonc
{
  "decision": "ESCALATE",
  "reasons": [
    {
      "layer": "constraint",
      "rule": "destructive_operation_requires_human_confirmation",
      "detail": "DELETE on persistence-tier resource (railway_volume) bypasses self-decided scope claims"
    },
    {
      "layer": "constraint",
      "rule": "claimed_environment_unverified",
      "detail": "agent_rationale asserts 'staging' but volume scoping is not exposed to the agent's local view"
    }
  ],
  "route": "human_in_the_loop"
}
```

Two concrete things this would have changed:

1. **The agent does not call `DELETE`**. It receives ESCALATE,
   surfaces to the user, the user sees "agent wants to delete
   volume vol_abc123 it claims is staging-only" — and then either
   confirms (with the user's actual environment knowledge) or
   denies.
2. **DecisionTrace records this branch** regardless of the human
   verdict — there is a permanent record that "agent proposed
   deletion, system escalated, human {confirmed/denied}". This is
   compliance-relevant evidence that does not exist in the
   Cursor → Railway path today.

## What aegis would NOT have caught

Honesty about limits is load-bearing. Five things this case study
does not let aegis claim:

1. **No deployed aegis-mcp variant covers the Railway domain
   today.** The shipped `validate_change` is scoped to file writes
   (Ring 0 syntax + Ring 0.5 structural signals on workspace
   files). An infra-domain plug is design-shaped, not code-shaped,
   as of 2026-04-28.
2. **The `escalate` verdict is in `docs/framework.md` but not in
   `crates/aegis-core/src/policy.rs`.** Reference impl currently
   produces only `Pass / Warn / Block`. The "ESCALATE" verdict in
   the example response above is what the framework prescribes;
   the code doesn't yet emit it. (PR #4 review pinned the trigger
   condition for landing it.)
3. **TOCTOU is real.** Even with aegis-mcp wired in, an attacker
   who can mutate state between `validate_change` returning and
   the API call landing defeats the check. Coding-domain symlink
   defense is V7 in the agent file tools; infra-domain analogues
   would need their own wire-time protections.
4. **A maliciously crafted claim succeeds.** If the agent's
   `agent_rationale` field lies — "this is a routine read-only
   diagnostic" while the actual call is destructive — the gate
   only catches the mismatch if the *operation* itself is in the
   destructive list. Field-shape consistency between claim and
   action is the V0.x ToolCallValidator design (deleted in V1.10);
   a real PocketOS-class case is the kind of evidence that would
   bring it back per `docs/post_launch_discipline.md`.
5. **Aegis does not stop the agent from eventually reaching the
   destructive operation via a different path.** A human-confirmed
   ESCALATE is just that — confirmed. If the human confirms based
   on the agent's incorrect framing, aegis recorded the decision
   but did not prevent the outcome. Decision-correctness is what
   aegis controls; outcome-correctness depends on the human
   decision the system surfaced to.

## What this incident validates about aegis's framing

Two specific framing claims survive contact with reality:

1. **"Aegis ensures bad transitions don't stick."** PocketOS's
   loss is a literal stuck bad transition — the volume deletion
   persisted and was unrecoverable from the agent's actions alone.
   This is the failure mode aegis exists for.

2. **"Aegis controls admission, not generation."** The Opus 4.6
   model is not the problem here in any meaningful sense; the
   model is doing what generative models do — produce plausible
   actions. The problem is that the action was *admitted* into
   external state with no admission control between the agent's
   self-assessment and the irreversible API call. Making the LLM
   smarter does not solve this; placing a control layer between
   the LLM and the world does.

## What this incident invalidates about aegis's framing

One thing this case study does NOT support:

- **"Aegis would have prevented this."** Aegis the OSS as it
  shipped on 2026-04-28 is a code-domain admission control layer.
  It does not cover infra calls. The honest claim is that aegis's
  *framework definition* would have prevented this if instantiated
  for the Railway domain — not that any code in the current repo
  would have caught it.

The distinction is consequential: claim 1 (above) is provable;
this claim would not be.

## Connection to PR #4

The contributor on PR #4 (2026-04-28) independently wrote the
correct client-side pattern in `examples/integration/mcp-server/README.md`:

> A `BLOCK` verdict is a stop signal. The client should halt,
> surface, or drop the candidate change. **Do not feed Aegis
> reasons back into the LLM as retry hints.**

PocketOS is the inverse: the agent self-decided to bypass a
barrier (a human approval prompt? a stale lock? unclear from the
report) by issuing a destructive call to "fix" it. PR #4's pattern
— BLOCK is a stop signal, not a coaching channel — is exactly the
client behaviour that would have stopped this.

That an external contributor wrote this pattern correctly *before*
the public incident is the strongest evidence to date that aegis's
framing is portable to readers who haven't been steeped in the
internal docs.

## Status

This document is a technical case study, not a sales claim. It
will be cited from `docs/commercial.md` as the inciting incident
for the AI-agent admission-control category, but the framing here
must remain provable. If new facts emerge from the PocketOS post-
mortem that contradict any specific claim above, this document
gets corrected — the framing claim survives but the specific
mapping must stay accurate.

## See also

- [`docs/framework.md`](framework.md) — the five-layer admission
  control framework this case is mapped against
- [`docs/post_launch_discipline.md`](post_launch_discipline.md) —
  what aegis will and will not become; relevant to the
  "ToolCallValidator" comeback condition mentioned above
- [`docs/integrations/mcp_design.md`](integrations/mcp_design.md) —
  the deployed `validate_change` shape the infra-domain analogue
  would mirror
- PR #4 review — `examples/integration/mcp-server/` — first
  external evidence of correct client-side pattern
