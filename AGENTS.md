# Instructions for AI coding agents (Claude Code, Cursor, Aider, etc.)

This file tells you, the AI agent, how to help a user adopt Aegis
in **their** project. The user almost certainly typed something like
"add Aegis to my repo" — they will not read installation docs. You
will. Follow the commands here verbatim; don't paraphrase, don't skip.

If anything in this file conflicts with your interpretation of
README.md, follow this file. README is for human evaluators; this
file is for you.

---

## What Aegis is, in one paragraph

A behavior harness for LLM-driven workflows. It rejects code-state
transitions that worsen structural metrics, blocks syntax/cycle
violations at write time, and emits a machine-readable trace of
every gate decision. **It does not generate code. It does not
optimize behavior. It does not retry on failure.** You and the
user's existing tools handle generation; Aegis sits as a side-channel
enforcement layer.

Full design context: [`README.md`](README.md) and
[`docs/v1_validation.md`](docs/v1_validation.md). Read those when
the user asks "what does this thing actually do".

---

## Setup — the canonical install sequence

Run these in order from the user's home or workspace dir. **Do not
combine, do not reorder, do not skip the verification step.**

```bash
# 0. Prerequisites — check before installing.
python --version           # need 3.10+
git --version              # any recent
cargo --version || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source "$HOME/.cargo/env"

# 1. Clone, enter, venv.
git clone https://github.com/wei9072/aegis ~/aegis
cd ~/aegis
python -m venv .venv
source .venv/bin/activate

# 2. Single-step install — builds the Rust extension via maturin AND
# installs the Python packages AND deps AND registers the `aegis` CLI.
# Takes 30s-2min on first run (Rust compilation), <5s on subsequent.
pip install -e .

# 3. VERIFY (do not skip — confirms the install worked end-to-end).
python examples/00_quickstart.py
```

Optional extras for specific scenarios:

```bash
pip install -e ".[dev]"       # adds pytest for running tests
pip install -e ".[mcp]"       # adds the MCP SDK for `aegis-mcp` server
pip install -e ".[dev,mcp]"   # both
```

Expected output of step 3 includes the lines `ALLOWED → ...` and
`BLOCKED → RuntimeError: ...`. If you don't see both, the install
is broken — diagnose before proceeding to integration. Common causes:

- `cargo: command not found` → restart shell or `source "$HOME/.cargo/env"`. The Rust toolchain is required for step 2 (PyPI wheels coming later — see issue tracker).
- `ImportError: dynamic module does not define module export function` → re-run step 2 (Rust extension may have been left in inconsistent state).
- `ModuleNotFoundError: No module named 'aegis'` → check that `python` resolves to the venv's Python (`which python`); if not, re-run step 2 with the venv active.

After step 3 passes, Aegis is installed and the `aegis` CLI is on
PATH. The Python package and Rust extension are both in the venv;
`python -c "from aegis import _core"` works. You're ready to integrate.

---

## Integration — pick exactly one for the user's situation

Ask the user (or infer from their workflow) which boundary they want
Aegis to enforce at:

| If the user... | Use this path |
| :--- | :--- |
| Has a personal project, wants `git commit` to block bad changes | **A. Pre-commit hook** |
| Has a team repo with PRs, wants reviewers to see Aegis status | **B. GitHub Action** |
| Is in Cursor / Claude Code / has their own LLM agent loop | **C. Wrap their LLM with `LLMGateway`** |

Don't stack paths until the user has lived with one for at least a
day. They are complementary, but layering before the basics work is
how you build untrustable setups.

### Path A — Pre-commit hook

Drop this verbatim into the user's project at `.git/hooks/pre-commit`,
then `chmod +x .git/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
set -euo pipefail
files=$(git diff --cached --name-only --diff-filter=ACM | grep '\.py$' || true)
[ -z "$files" ] && exit 0
source "$HOME/aegis/.venv/bin/activate"
echo "$files" | (cd "$HOME/aegis" && PYTHONPATH=. xargs -I{} python -m aegis.cli check "$OLDPWD/{}")
```

Verify by triggering a known-fail (paste this into the user's repo
to confirm the hook works, then revert):

```bash
echo "def add(a, b)" > /tmp/aegis_test_broken.py
cp /tmp/aegis_test_broken.py ./broken_test.py
git add broken_test.py
git commit -m "should be blocked"   # expect: rejected with [Ring 0] message
git restore --staged broken_test.py
rm broken_test.py
```

Reference: [`docs/integrations/git_pre_commit.md`](docs/integrations/git_pre_commit.md)
(read this if the user asks for variations).

### Path B — GitHub Action

Add `.github/workflows/aegis.yml` to the user's repo. The full
workflow is in [`docs/integrations/github_action.md`](docs/integrations/github_action.md);
copy it verbatim. Then tell the user to mark the "Aegis Ring 0"
check as required in branch protection (you can't do this for them
— it's a GitHub UI action).

### Path C — Wrap the user's LLM in `LLMGateway`

The integration is one Python class + replacing one call site. The
canonical template is [`examples/00_quickstart.py`](examples/00_quickstart.py).
Adapted for the user's existing LLM:

```python
from aegis.agents.llm_adapter import LLMGateway

class UsersLLM:
    """Wrap whatever the user is calling today."""
    def generate(self, prompt: str, tools=None) -> str:
        return existing_llm_call(prompt)   # the user's current call

gateway = LLMGateway(llm_provider=UsersLLM())

# Replace direct LLM calls with this:
safe_response = gateway.generate_and_validate(prompt, max_retries=1)

# Read the trace to surface decision events to the user / log:
for event in gateway.last_trace.events:
    print(f"{event.layer}:{event.decision} {event.reason}")
```

If the user has a multi-turn refactor loop (not just single
completions), use `pipeline.run()` instead of `LLMGateway` — see
[`examples/01_pipeline_basic.py`](examples/01_pipeline_basic.py)
and [`examples/03_custom_verifier.py`](examples/03_custom_verifier.py).

**For Cursor / Claude Code users specifically:** there's a fourth
path — the MCP server. Install the optional `mcp` extra
(`pip install -e ".[mcp]"`) and run `aegis-mcp`. Configure the
client per
[`docs/integrations/mcp_design.md`](docs/integrations/mcp_design.md);
the agent can then call `validate_change(path, new_content,
old_content?)` mid-loop and get a structured verdict back. Only
`validate_change` exposed in V0.x — if the user needs the other
tools (`validate_diff`, `get_signals`), tell them to file an issue.

---

## Rules you must follow when working on / with Aegis

These are the project's framing constraints. PRs that violate them
get reverted; suggestions that violate them get rejected. **Do not
propose any of these to the user.**

1. **Do not add automatic retry.** If `gateway.generate_and_validate()`
   raises, the agent / human decides what to do next. Aegis does not
   loop until it gets a passing response.
2. **Do not feed verifier output back into the LLM prompt.** The
   verdict is observation, not coaching. If the user wants the LLM
   to "know what was wrong", they edit the prompt themselves —
   you do not auto-construct hint strings from `gateway.last_trace`.
3. **Do not propose new gates / decision patterns / verifier types
   without checking [`docs/post_launch_discipline.md`](docs/post_launch_discipline.md).**
   That file lists 5 explicit categories of features deferred until
   real-user demand justifies them.
4. **Do not propose extracting `aegis-core` as a generic framework.**
   See [`docs/future_abstraction.md`](docs/future_abstraction.md) —
   three trigger conditions must be met first; they aren't yet.
5. **Do not modify `tests/test_task_verifier.py::test_task_verdict_has_no_feedback_field`.**
   It enforces rule 2 structurally.

If a user request implies any of these, stop and explain that the
request would change Aegis from a constraint system into an optimizer,
and ask whether they want a discussion thread instead of a PR.

---

## Where things are

Cheatsheet for "I need to find X in this repo":

| You need... | Look at... |
| :--- | :--- |
| Understand what Aegis is for humans | [`README.md`](README.md) |
| Run a no-API-key demo | [`examples/00_quickstart.py`](examples/00_quickstart.py) |
| Real-LLM single-call example | [`examples/02_gateway_single_call.py`](examples/02_gateway_single_call.py) |
| Multi-turn refactor with verifier | [`examples/01_pipeline_basic.py`](examples/01_pipeline_basic.py), [`examples/03_custom_verifier.py`](examples/03_custom_verifier.py) |
| Read the decision trace | [`examples/04_read_decision_trace.py`](examples/04_read_decision_trace.py) |
| Add a new LLM provider | mirror [`aegis/agents/groq.py`](aegis/agents/groq.py) — subclass `OpenAIProvider` if OpenAI-compatible, else implement the `LLMProvider` Protocol from scratch |
| Add a new scenario | new dir under [`tests/scenarios/`](tests/scenarios/); copy structure from `tests/scenarios/syntax_fix/` |
| Run all tests | `PYTHONPATH=. python -m pytest tests/ -q` (251 should pass) |
| Run cross-model evidence sweep | `python scripts/v1_validation.py` |
| Aggregate sweep results | `python scripts/v1_aggregate.py` |
| Understand the V1.6 evidence | [`docs/v1_validation.md`](docs/v1_validation.md) |
| Understand what V2 looks like | [`docs/gap3_control_plane.md`](docs/gap3_control_plane.md) (design only, not implemented) |
| Understand what's deferred | [`docs/post_launch_discipline.md`](docs/post_launch_discipline.md) |

---

## When things go wrong

If a user reports something Aegis blocks that they think shouldn't
be blocked:

1. Run `python examples/04_read_decision_trace.py` (or the user's
   equivalent) to capture the full trace.
2. Identify which gate fired (the `layer` field).
3. **Don't** propose modifying the gate logic to let it pass.
4. **Do** open an issue at https://github.com/wei9072/aegis/issues
   with: their input, the trace, and why they think the block was
   wrong. This is V0.x evidence — false positives + false negatives
   both refine the system.

If a user reports Aegis didn't block something it should have:

1. Same — capture the trace and what they expected.
2. **Don't** propose adding a new gate inline.
3. **Do** open an issue. The fix path is "add a scenario that
   reproduces this, then design the gate that catches it" — not
   reactive patches.

---

## Status (so you don't promise things that aren't real)

What works in V0.x: [`README.md`'s Status section](README.md#status).
Don't promise the user features marked 🟡 or ❌ — they don't exist
yet, and "I'll just build a quick version" is exactly what
[`docs/post_launch_discipline.md`](docs/post_launch_discipline.md)
is meant to prevent.

If you're unsure whether a capability exists: grep the codebase
before claiming it does. The code is the source of truth; this
file and README can drift, the code can't.
