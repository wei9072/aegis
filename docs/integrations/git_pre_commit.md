# Git pre-commit hook

The fastest way to add Aegis to your workflow: a shell hook that
runs every time you `git commit`. If your AI tool just produced
something that fails Ring 0 (syntax error) or that you want to
check for structural signals, the commit is blocked and you see
why.

**Workflow change for you:** install `aegis` once, drop the hook
into your repo. After that, zero — Aegis only surfaces when it
actually catches something.

---

## One-time setup

Build and install the CLI somewhere on `$PATH`. Either install
system-wide via cargo:

```bash
git clone https://github.com/wei9072/aegis ~/aegis
cd ~/aegis
cargo install --path crates/aegis-cli
```

Or grab a pre-built binary from
[GitHub Releases](https://github.com/wei9072/aegis/releases) once
V2.0 publishes (Linux x86_64/aarch64, macOS x86_64/aarch64,
Windows x86_64).

Verify:

```bash
aegis languages              # prints the 10 supported languages
aegis check Cargo.toml       # zero-signal smoke test on any file
```

---

## Per-project hook

In each project where you want Aegis enforcement, drop this into
`.git/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
# .git/hooks/pre-commit — block commits that fail Aegis Ring 0.
#
# Catches: syntax errors in any of 10 supported languages.
# Runs in <1 second on small diffs.

set -euo pipefail

# All extensions Aegis can parse.  Run `aegis languages` for the
# live registry.
EXT_PATTERN='\.(py|pyi|ts|tsx|mts|cts|js|mjs|cjs|jsx|go|java|cs|php|phtml|swift|kt|kts|dart|rs)$'

files=$(git diff --cached --name-only --diff-filter=ACM | grep -E "$EXT_PATTERN" || true)
[ -z "$files" ] && exit 0

# `aegis check` exits non-zero on Ring 0 violations.
echo "$files" | xargs aegis check
```

Make it executable:

```bash
chmod +x .git/hooks/pre-commit
```

That's it. From now on, any commit that touches a supported source
file runs through Aegis Ring 0 first.

---

## What it catches

Run `aegis check` on a file with a syntax error:

```
broken.py:1: [Ring 0] Syntax error detected: expected ':' (line 1)
```

The commit fails with the same message. Fix the file, re-add it,
re-commit.

---

## What it does NOT catch

The pre-commit hook only runs Ring 0 (per-file syntax check). It
does **not**:

- Run cost-aware regression detection (would need a HEAD-vs-staged
  comparison; achievable with a slightly fancier hook that runs
  `aegis check` on both versions and diffs the signals — open an
  issue if you want a worked example).
- Trigger on Ring 0.5 signal values alone — `fan_out=22` doesn't
  fail the hook on its own. Use the [MCP server](mcp_design.md)
  for cost-regression enforcement against a baseline.
- Run any LLM-backed gate. The pre-commit hook is deterministic +
  zero-API-key by design.

For multi-turn refactor protection (where the LLM iterates and
each iteration's cost should be compared to the previous), see
the [MCP integration](mcp_design.md) — that's where the full
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

## Variations

### JSON output for tooling

```bash
echo "$files" | xargs aegis check --json > .aegis-precommit.json
```

Each entry has `ok: bool`, `signals: [{name, value}, ...]`. Pipe
it into `jq` if you want custom failure logic (e.g. "only fail if
fan_out > 30").

### Single-language-only project

Trim the extension pattern to just what you ship — e.g. for a
TypeScript-only repo:

```bash
EXT_PATTERN='\.(ts|tsx|mts|cts)$'
```

No other change needed.
