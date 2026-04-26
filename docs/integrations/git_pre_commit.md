# Git pre-commit hook

The fastest way to add Aegis to your workflow: a shell hook that
runs every time you `git commit`. If your AI tool just produced
something that fails Ring 0 (syntax error, circular dependency,
etc.), the commit is blocked and you see why.

**Workflow change for you:** install the hook once. After that,
zero — Aegis only surfaces when it actually catches something.

---

## V0.x install workaround

Aegis doesn't have a PyPI package yet (tracked at
[`docs/launch/issue_rust_build_friction.md`](../launch/issue_rust_build_friction.md)).
Until it does, point the hook at a local clone of the repo via
an environment variable.

### One-time setup

```bash
# 1. Clone Aegis somewhere stable (only if you don't have it already)
git clone https://github.com/wei9072/aegis ~/code/aegis
cd ~/code/aegis
python -m venv .venv && source .venv/bin/activate
pip install maturin click google-genai prompt_toolkit
cd aegis-core-rs && maturin develop --release && cd ..

# 2. Tell your shell where Aegis lives
echo 'export AEGIS_HOME=~/code/aegis' >> ~/.bashrc
source ~/.bashrc
```

### Per-project hook

In each project where you want Aegis enforcement, drop this into
`.git/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
# .git/hooks/pre-commit — block commits that fail Aegis Ring 0.
#
# Catches: syntax errors, circular dependency introductions, and
# anything else Ring 0 enforces. Runs in <1 second on small diffs.

set -euo pipefail

if [ -z "${AEGIS_HOME:-}" ]; then
  echo "AEGIS_HOME not set; skipping Aegis check."
  exit 0
fi

# Only check Python files that are about to be committed.
files=$(git diff --cached --name-only --diff-filter=ACM | grep '\.py$' || true)
if [ -z "$files" ]; then
  exit 0
fi

# Activate the Aegis venv and run Ring 0 enforcer on each file.
source "$AEGIS_HOME/.venv/bin/activate"
echo "$files" | (cd "$AEGIS_HOME" && PYTHONPATH=. xargs -I{} python -m aegis.cli check "$OLDPWD/{}")
```

Make it executable:

```bash
chmod +x .git/hooks/pre-commit
```

That's it. From now on, any commit that touches a `.py` file runs
through Aegis Ring 0 first.

---

## What it catches

Run `aegis check` on a file with a syntax error and you'll see
output like:

```
broken.py:1: [Ring 0] Syntax error detected: expected ':' (line 1)
```

The commit fails with the same message. Fix the file, re-add it,
re-commit.

For a commit that introduces a circular dependency between modules:

```
moduleA.py: [Ring 0] Circular dependency: moduleA → moduleB → moduleA
```

---

## What it does NOT catch (yet)

The pre-commit hook today only runs Ring 0 (single-file structural
checks). It does **not**:

- Run cost-aware regression detection (would need a HEAD-vs-staged
  comparison; tracked as future work).
- Catch policy violations (e.g. fan_out spikes); only the `block`
  half of PolicyEngine, not the `warn` half.
- Run any LLM-backed gate (Tier-2 ToolCallValidator, IntentBypass).

For multi-turn refactor protection, see the
[MCP integration design](mcp_design.md) — that's where the full
pipeline (with regression rollback) belongs, because it's
turn-by-turn, not commit-by-commit.

---

## Disabling temporarily

If you need to commit something Aegis blocks (and you've thought
about it), use the standard git escape hatch:

```bash
git commit --no-verify
```

This bypasses ALL hooks for that commit. Use sparingly — every
`--no-verify` is evidence that either Aegis was wrong, or you were.
Both are worth recording somewhere (issue, follow-up commit
message).

---

## Future cleanup

When Aegis ships PyPI wheels, the hook collapses to:

```bash
#!/usr/bin/env bash
files=$(git diff --cached --name-only --diff-filter=ACM | grep '\.py$' || true)
[ -n "$files" ] && echo "$files" | xargs aegis check
```

No `AEGIS_HOME`, no venv activation, no PYTHONPATH. This is the
target shape; the install-friction issue is what's keeping us
honest about V0.x.

If you hit problems with the workaround above, please report them
at [`docs/launch/issue_rust_build_friction.md`](../launch/issue_rust_build_friction.md)
— specific OS / Python / shell reports help us prioritise the
PyPI work.
