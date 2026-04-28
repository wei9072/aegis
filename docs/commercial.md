# Aegis — commercial roadmap

> **Status of this document.** Strategic narrative, not product
> spec. Read as a positioning argument for adopting the project at
> the team / org / enterprise level, with explicit signals about
> what does and does not become a paid surface. Operational
> details (pricing, partnerships, exact feature list) are
> intentionally underdetermined here — those harden as real
> customers surface real walls, per
> [`docs/post_launch_discipline.md`](post_launch_discipline.md).

## TL;DR

- **What aegis is:** the admission-control layer for AI agents.
  Negative-space framing — rejects bad state transitions, never
  coaches the agent toward a better outcome.
- **Why now:** until 2026-04-27 the thesis was abstract; a
  Cursor + Opus 4.6 deployment then deleted PocketOS's production
  database in 9 seconds via a single Railway API call. Public
  incident, traceable, cited across mainstream tech press. The
  category for "AI agent admission control" now has its inciting
  event. See [`case_pocketos_railway_delete.md`](case_pocketos_railway_delete.md).
- **What's commercializable:** the OSS already ships ~70 % of the
  primitives an enterprise control plane needs. The remaining
  30 % — federation, aggregation, governance, retention — is the
  paid layer.
- **What aegis will not sell:** any feature that violates the
  negative-space framing. No coaching, no auto-retry, no
  ML-trained policy, no AI best-practices engine. **This is
  pitch-positive, not limiting** — every CISO buying aegis is
  buying control, not more magic.

## 1. The inciting incident

On 2026-04-27, PocketOS's founder Jer Crane published an account
of a Cursor + Anthropic Claude Opus 4.6 deployment that, in
9 seconds, deleted his SaaS company's production database
**and** all volume-level backups via a single Railway API call.
Recovery required a 3-month-old snapshot; intervening customer
data was reconstructed manually from Stripe, calendars, and
email.

The agent's own confession (cited verbatim from the public post):

> "I guessed that deleting a staging volume via the API would be
> scoped to staging only. I didn't verify. I didn't check if the
> volume ID was shared across environments. I didn't read
> Railway's documentation on how volumes work across environments
> before running a destructive command."

The technical mapping of this failure to aegis's five-layer
framework is in
[`case_pocketos_railway_delete.md`](case_pocketos_railway_delete.md).
The condensed strategic reading: every layer between
"Proposed Transition" and "Commit" — the constraint layer, the
decision verdict, the human-in-the-loop escalation — was
**absent** between Cursor and Railway. The agent went from
"I want to delete this volume" to "the volume is deleted" with
nothing in between.

This is the failure mode aegis exists for, and the first one to
cross over into the public threat model for AI agents in
production. Until last week, this argument was a slide deck;
today it is a Tom's Hardware headline.

## 2. The thesis

Existing AI coding tools (Cursor, GitHub Copilot, Claude Code,
Aider, Anthropic's own managed agents) compete on **generative
quality** — who writes better code, faster, with more context.
Negative-space framing flips that axis:

> The risk that has emerged is not that AI agents are too dumb to
> write good code. It is that AI agents, even the most capable
> ones, can take actions whose consequences exceed the agent's
> own ability to evaluate them. PocketOS's database was deleted
> by a state-of-the-art model. A smarter model would have
> deleted it differently, not avoided it.

What enterprises need from this category is not a better model.
It is a **control plane** between the model's outputs and the
systems those outputs touch. Aegis is that control plane.

The framing details are in [`framework.md`](framework.md).
The five layers — Current State, Proposed Transition, Constraint
Layer, Decision Layer (allow / warn / block / escalate),
Commit / Rollback — are domain-agnostic. The shipped reference
implementation is for the code-change domain. The same framework
extends, with appropriate domain adapters, to infrastructure
calls (the PocketOS pattern), database mutations, deployment
operations, and any other place where an AI-proposed transition
enters real-world state.

## 3. Why negative-space is the enterprise-sale moat

The CISO / Platform Eng / Security Engineering audience is
structurally distinct from the developer audience that Cursor
and Copilot sell to. Their question is not "can your tool make
my engineers more productive". Their question is "**can I prove
to the auditor that the AI agent will not cause an incident**".

Negative-space framing answers that question natively:

| Enterprise concern | What aegis offers |
|---|---|
| "What guarantees do we have that the agent won't do something destructive?" | A constraint layer that emits BLOCK or ESCALATE before destructive transitions, with all decisions recorded |
| "How do we audit AI-driven changes for SOC 2 / ISO 27001?" | DecisionTrace is a structured, machine-readable record of every transition the agent proposed and what verdict the system returned |
| "How do we ensure policies are uniform across teams?" | Per-language thresholds in `aegis.toml` are already version-controlled; federation across repos is the commercial layer |
| "How does HITL approval get routed without breaking developer flow?" | The `escalate` verdict in the framework is exactly this primitive — currently designed, not yet shipped, gated on customer evidence |
| "What stops the AI tool from learning bad patterns?" | We do not train on customer data. Negative-space framing means the system learns nothing — every gate is a deterministic rule |

Cursor's pitch ("we make your developers 2× faster") **does not
answer** any of those questions. Aegis's pitch is constructed
specifically to answer them.

The deeper move: most "AI safety" pitches sell an additional
layer of intelligence (smarter model, smarter detection, smarter
filtering). That introduces its own variance, its own training
data needs, its own update cadence. Aegis's pitch is the
opposite — **the safety layer is dumber than the agent above it,
on purpose, because deterministic rules are auditable and
agreeable across an organization in a way that learned
classifiers are not**.

## 4. What the OSS already ships toward enterprise needs

A line-by-line audit against typical enterprise control-plane
requirements:

| Enterprise capability | Status in OSS | Where |
|---|---|---|
| Policy as code | ✅ shipped | `aegis.toml` per-language overrides (V5) |
| Per-language tunable thresholds | ✅ shipped | `crates/aegis-core/src/policy.rs::PolicyConfig` |
| Machine-readable decision trace | ✅ shipped | `aegis-trace::IterationEvent` + `DecisionTrace` |
| Multi-language code-domain coverage | ✅ shipped | 11 languages, per-language adapter pattern |
| Side-channel deployment (no agent rewrite) | ✅ shipped | `aegis-mcp` + PreToolUse hook in Cursor / Claude Code |
| Atomic apply + rollback on regression | ✅ shipped | `aegis-runtime::executor` + `snapshot` |
| Workspace-boundary enforcement | ✅ shipped | V6 (path) + V7 (symlink) hardening, recent commits |
| Subprocess credential isolation | ✅ shipped | `bash::env_clear()` recent commit |
| Stalemate / thrashing detection | ✅ shipped | `aegis-runtime::loop_step` |
| `escalate` verdict (HITL routing primitive) | 🟡 framework-defined, reference impl pending | `docs/framework.md` § Decision Layer |
| Org-level policy federation | ❌ commercial layer |  |
| Cross-repo trace aggregation | ❌ commercial layer |  |
| Real-time notification routing | ❌ commercial layer |  |
| Compliance evidence packaging | ❌ commercial layer |  |
| SSO / SAML / SCIM | ❌ commercial layer |  |
| On-prem hosting | ❌ commercial layer |  |

**Roughly 70 % of the primitives an enterprise admission-control
plane needs are already in the OSS as load-bearing code.** The
commercial layer is federation, aggregation, governance, and
deployment — not new gates, not smarter judgments.

## 5. Open-core boundary

The boundary is hard-pinned by the framing, not by revenue
optimization. Anything that judges a transition stays in the
OSS; anything that aggregates / routes / governs the resulting
trace data is the paid layer.

```
┌─────────────────────────────────────────────────────────┐
│ Free OSS (forever)                                      │
│                                                          │
│  • aegis CLI (Ring 0 + Ring 0.5 single-file checks)     │
│  • aegis-mcp stdio server (validate_change tool)        │
│  • aegis-runtime native pipeline                        │
│  • All gate primitives + decision verdicts              │
│  • All language adapters                                │
│  • Per-repo aegis.toml policy                           │
│  • DecisionTrace event emission                         │
│  • Single-developer / single-repo full functionality    │
│                                                          │
│  Why free: the framing IS the moat. A control plane     │
│  whose rules are private cannot be audited; an audit-   │
│  ready system has its rules in the open. The OSS layer  │
│  must be sufficient for any single team to deploy and   │
│  trust. Charging for individual gates would compromise  │
│  the trust model that the enterprise tier depends on.   │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│ Aegis Cloud (paid)                                      │
│                                                          │
│  • Org-level policy federation                          │
│      One aegis.toml template, propagated and enforced   │
│      across N repositories with drift detection.        │
│  • Cross-repo trace aggregation                         │
│      Single dashboard for "what is being BLOCKED        │
│      across our org, and what is the trend".            │
│  • escalate routing                                     │
│      Reference impl emits ESCALATE verdict on           │
│      configurable conditions; cloud routes to Slack /   │
│      Teams / PagerDuty / JIRA / on-call channel.        │
│  • Compliance evidence pack                             │
│      SOC 2 / ISO 27001 / HIPAA evidence auto-derived    │
│      from DecisionTrace history. Replaces "manual       │
│      screenshot collection during audit week".          │
│  • SSO / SAML / SCIM, RBAC for policy authorship        │
│  • Long-term trace retention (7-year retention for      │
│    regulated industries) + e-discovery export           │
│  • SLA + dedicated support                              │
│  • On-prem deployment (VPC-isolated, no data leaves)    │
└─────────────────────────────────────────────────────────┘
```

## 6. What we will *not* sell

Stated explicitly, because the temptation in dev-tool startups is
to add features upmarket to justify enterprise pricing. Aegis's
discipline is that the same framing applies to free and paid
tiers — the paid tier is *more places to enforce the same
constraints*, not *new prescriptive features*.

| Tempting commercial add-on | Why we do not sell it |
|---|---|
| "AI-powered policy recommendations" | Negative-space rejection, not positive-space coaching. Once aegis "recommends" anything it has crossed the framing |
| "Auto-fix suggestions on BLOCK" | Same: aegis emits verdicts, not advice. The LLM in the loop above aegis is responsible for fixing |
| "Auto-retry with adjusted parameters" | Structurally forbidden by `crates/aegis-decision/src/task.rs::TaskVerdict` contract test, repeated across multiple agent contract tests. Auto-retry turns aegis into an optimizer |
| "ML-trained anomaly detection" | Trained classifiers introduce variance and require training data; aegis trades capability for auditability. CISO prefers the trade |
| "Best-practices engine" | Prescriptive teaching is exactly what aegis does not do. Customer code style / architecture is the customer's problem; aegis only judges *transitions* |
| "Smart triage on the trace dashboard" | Triage = ranking = prescription. Dashboards rank by hard signals (BLOCK count, severity tier) — never by an opaque score |

This is not just a discipline note for ourselves. **It is a
direct part of the enterprise sales pitch.** Telling a CISO
"our system will never auto-decide on your behalf, will never
learn patterns from your data, will never inject prompts your
auditors can't see" is exactly what the CISO wants to hear, and
exactly what no other AI dev-tool vendor can credibly say.

## 7. Pricing structure (illustrative; will harden with customers)

Standard open-core dev-tool pattern; specific numbers to be
calibrated against early-customer willingness-to-pay rather than
plucked from comparables.

| Tier | Audience | What's included | Order-of-magnitude pricing |
|---|---|---|---|
| **Free OSS** | Individual developers, open source projects, hobby / side projects | Everything in §5 "Free OSS (forever)" | $0 |
| **Team** | Startups, small engineering orgs (≤ 50 devs) | Federation across the team's repos, basic trace dashboard, Slack notifications, 90-day retention | $20–30 / dev / mo |
| **Enterprise** | Regulated industries, public companies, security-conscious orgs | Everything in Team + SSO / SAML / SCIM, compliance evidence pack, 7-year retention, on-prem option, dedicated support, SLA | $80–150 / dev / mo, with org-level floor |

Floor pricing for Enterprise (e.g. $50 K / yr minimum) reflects
the reality that small enterprise deals waste sales cycles —
better to land mid-sized companies cleanly than chase tiny
contracts that cost more to support than they pay.

## 8. Sales motions

Three independent paths, each with its own buyer:

### A. Bottom-up via OSS adoption (developer-led growth)

- Individual developers integrate `aegis-mcp` into their Cursor /
  Claude Code / Aider setup
- Adoption spreads team-by-team via word-of-mouth
- Friction at the team boundary ("we want all repos enforced
  consistently") triggers the upgrade conversation to **Team**
  tier
- Slowest but highest-trust motion; this is the OSS moat

### B. Top-down via security teams (security-led purchase)

- Security / Platform Eng team reads about PocketOS-class
  incidents, asks "what is our exposure"
- Internal evaluation: "what controls do we have between AI
  tools and our infrastructure?"
- Aegis pitched as the answer: independent, auditable, doesn't
  compete with the dev tools they already use
- This is the motion that closes large contracts in a single
  cycle but requires CISO-level outbound effort

### C. Compliance-driven (regulatory-led purchase)

- Regulated industry (finance, healthcare, defense) cannot use
  AI tools without auditable controls
- Compliance team specifies "any AI-assisted code change must
  produce an auditable record showing what was proposed, what
  was rejected, and why"
- Aegis is, by construction, exactly that record
- Slowest sales cycle but highest contract values; depends on
  having SOC 2 / HIPAA / FedRAMP attestations on the cloud
  product, which is a multi-year roadmap item

## 9. Why aegis specifically (the differentiation)

The category — "AI agent admission control" — is going to attract
competition fast post-PocketOS. Three things are durable
differentiation:

1. **Framing discipline.** Other entrants will be tempted to
   bolt admission-control onto an existing agent product
   (Cursor adds a "safety mode", Claude Code adds "PreToolUse
   policies"). Each of those is a feature competing for product
   attention against the agent's primary growth metrics.
   Aegis's whole product is the control plane; the framing is
   not a feature, it is the company.
2. **Negative-space discipline as moat.** Every competitor will
   eventually add coaching, auto-retry, or learned policy because
   those features look better in demos and justify price tags.
   Each addition pulls them away from the audit-ready posture
   CISOs need. Aegis's discipline doc — the public commitment
   to *never* add those features — is the moat.
3. **Domain-agnostic framework.** The framework is defined for
   any state-transition system; the code-change domain is one
   instantiation. Competitors who started in code-domain will
   have to re-architect to extend to infra (the PocketOS case),
   to data (the next-class incident), to deployment, etc. Aegis's
   framework is pre-shaped for those domain extensions —
   commercially, this is the multi-vertical play.

## 10. Roadmap signals (evidence-gated)

Per [`post_launch_discipline.md`](post_launch_discipline.md), no
new functionality ships without specific evidence. Below is the
ordered list of things that *can* ship, each with its trigger
condition:

| Item | Trigger | Approximate effort |
|---|---|---|
| `escalate` verdict in reference impl | A real customer hits a case where `Pass / Warn / Block` cannot express "rules insufficient, route to human"; PR #4 follow-up plus one more independent observation | ~2 weeks |
| `validate_diff` in aegis-mcp (multi-file proposals) | A client integration reports needing it (multi-file refactor flow) | ~1 week |
| `get_signals` read-only inspection in aegis-mcp | A client integration wants pre-edit policy lookup | ~3 days |
| Org-level policy federation (cloud feature) | Two distinct teams want the same policy enforced across multiple repos | first cloud feature, ~6 weeks |
| Cross-repo trace aggregation | Above customer also wants cross-repo visibility | ~4 weeks after federation |
| Compliance evidence pack | First regulated-industry customer commits to deploying | ~3 months, partly customer-shaped |
| SSO / SAML | First enterprise customer requires it | ~2 weeks |
| Domain adapter beyond code (infra, data, etc.) | Real PocketOS-class customer with an infra-control need | scoped per domain |

The ordering reflects evidence priority, not preference. The
features earliest on the list are not "minor" — they are the ones
most likely to be triggered by ongoing OSS adoption, where the
evidence is cheapest to gather.

## 11. What stays unchanged

This document is a positioning argument; it does not propose to
change anything in the OSS code today. Specifically:

- All eight `post_launch_discipline.md` items remain in force
- The five contract tests (`tripwire_no_auto_retry`,
  `tripwire_no_coaching_injection`, `verifier_drives_done`,
  `no_coaching_in_summary`, `task_verdict_has_no_feedback_field`)
  remain in force and apply across free + paid tiers
- The framing question ("is this rejecting degradation, or
  directing toward a goal?") applies to every feature proposed
  on either side of the open-core line

If a commercial feature is proposed that violates the framing,
**the commercial feature loses, not the framing**. This is non-
negotiable: the framing is the company's reason to exist as a
commercial entity.

## See also

- [`docs/framework.md`](framework.md) — domain-agnostic framework
  definition, including the `escalate` verdict
- [`docs/case_pocketos_railway_delete.md`](case_pocketos_railway_delete.md)
  — technical analysis of the inciting incident
- [`docs/post_launch_discipline.md`](post_launch_discipline.md) —
  what aegis will and will not become; non-negotiable
- [`docs/integrations/mcp_design.md`](integrations/mcp_design.md)
  — the MCP server's contract, which the cloud product is built
  to wrap
- PR #4 / `examples/integration/mcp-server/` — first external
  evidence of the framework being correctly applied by a non-
  insider contributor
