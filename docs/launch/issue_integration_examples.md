# GitHub Issue draft — "Integration examples wanted (agent / CI)"

Copy the body below into a new GitHub issue. Suggested labels:
`good-first-issue`, `examples-wanted`, `help-wanted`.

---

**Title:** Integration examples wanted — wrap your agent / CI / IDE plugin in Aegis and tell us where it broke

---

## Body

The four scripts in [`examples/`](../../examples/) cover the core
library patterns (multi-turn pipeline, single-call gateway, custom
verifier, decision-trace consumption). They're deliberately
minimal — single-file, copy-runnable, no external integration.

The next layer of evidence the project needs is **integration
examples** — Aegis embedded inside something else. We'd like to
collect 3-5 of these as `examples/integration/` so future readers
can see realistic shapes:

### Wanted shapes

1. **Wrap an existing AI coding agent.** Aider, Claude Code,
   Cursor, custom orchestrators — anything that has its own
   plan-execute loop. Show how Aegis's gateway / pipeline plugs in
   as a control layer. Bonus points if the agent's behavior changes
   visibly when Aegis rejects a step.

2. **Run as a CI gate.** GitHub Actions / GitLab CI / Jenkins
   workflow that runs Aegis against an LLM-proposed PR diff before
   merging. Decision pattern → CI status check. Even a no-op skeleton
   is useful as a starting point.

3. **MCP server.** Aegis exposed as an MCP tool, so any
   MCP-supporting IDE (Claude Code, Cursor, etc.) can call it as a
   sandboxed code-modification layer. We've sketched this in the
   roadmap but no implementation yet.

4. **Pre-commit / lint integration.** No-LLM use of Aegis's
   structural checks (`Ring0Enforcer`, `SignalLayer`) as a
   pre-commit hook. Aegis already supports this via
   `examples/02_gateway_single_call.py`'s underlying APIs but a
   dedicated example would help.

5. **Your own domain.** If you're trying to use Aegis for something
   that isn't code-gen — database migration, config rollout, RL
   policy iteration, etc. — share the integration even if it's
   half-finished. See
   [`docs/future_abstraction.md`](../future_abstraction.md) for
   how non-code domains map to Aegis's primitives.

### How to contribute

- **Quick & dirty** — paste your script in this issue, even if it's
  rough. We'll figure out together whether it becomes an upstreamed
  example or stays as a reference.
- **Polished** — open a PR adding a directory under
  `examples/integration/<your-shape>/` with a README explaining the
  what / why / how-to-run.

### What we're NOT looking for

- Integrations that route Aegis output (verifier results, decision
  patterns) **back into the LLM prompt** as automatic retry hints.
  See [the negative-space framing](../v1_validation.md#framing--what-aegis-actually-is)
  and [`docs/gap3_control_plane.md`](../gap3_control_plane.md)'s
  Critical Principle — the system must reject degradation, not
  optimize toward goals via auto-retry.
- "I built a chatbot using Aegis" — Aegis isn't a chat surface, the
  CLI / API isn't designed for conversational loops.

### Why this is open-call

Aegis V1 + V1.5 + V1.6 evidence ([`docs/v1_validation.md`](../v1_validation.md))
shows the decision system fires correctly across 5 model families
on 4 built-in scenarios. What we don't have evidence for is whether
the integration story works in real workflows. That's a question
only external users can answer.

If you build an integration and it surfaces friction (Rust build
issues, API gaps, missing documentation), that's exactly the
feedback we need. Open follow-up issues freely.
