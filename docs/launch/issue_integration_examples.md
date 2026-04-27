# GitHub Issue draft — "Integration examples wanted (agent / CI)"

Copy the body below into a new GitHub issue. Suggested labels:
`good-first-issue`, `examples-wanted`, `help-wanted`.

---

**Title:** Integration examples wanted — wrap your agent / CI / IDE plugin in Aegis and tell us where it broke

---

## Body

V1.10 ships two binaries (`aegis` + `aegis-mcp`) and three
ready-to-use integration paths (pre-commit hook, GitHub Action,
MCP server) — see [`docs/integrations/`](../integrations/) for
paste-ready templates.

The next layer of evidence the project needs is **real-world
integration examples** — Aegis embedded inside something
non-template that exercises one of the paths under real traffic.
We'd like to collect 3-5 of these so future readers can see
realistic shapes:

### Wanted shapes

1. **Wrap an existing AI coding agent (real PreToolUse / MCP).**
   Claude Code, Cursor, Aider, custom orchestrators — anything
   that calls `aegis-mcp validate_change` mid-loop or runs the
   PreToolUse hook on every Edit/Write. Bonus points if the
   agent's behavior changes visibly when Aegis rejects a step.

2. **Run as a CI gate on a real repo.** Take the GitHub Action
   template from
   [`docs/integrations/github_action.md`](../integrations/github_action.md)
   and report what broke / what surprised you on a real PR
   workload.

3. **Pre-commit hook on a polyglot repo.** Take the template
   from
   [`docs/integrations/git_pre_commit.md`](../integrations/git_pre_commit.md)
   and report whether the multi-language coverage is right for
   your repo, what false positives you hit, etc.

4. **Your own domain.** If you're trying to use Aegis for
   something that isn't code-gen — database migration, config
   rollout, RL policy iteration, etc. — share the integration
   even if it's half-finished. See
   [`docs/future_abstraction.md`](../future_abstraction.md) for
   how non-code domains map to Aegis's primitives.

### How to contribute

- **Quick & dirty** — paste your wrapper / config in this issue,
  even if it's rough. We'll figure out together whether it
  becomes an upstreamed template or stays as a reference.
- **Polished** — open a PR adding a section to the relevant
  doc under [`docs/integrations/`](../integrations/) with a
  worked example.

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
