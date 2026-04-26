# GitHub Action / CI gate

Block PR merges when Aegis Ring 0 fails. Same enforcement shape as
the [pre-commit hook](git_pre_commit.md), but at the team boundary
instead of the developer boundary — every PR gets an Aegis check
status visible to reviewers.

**Workflow change for your team:** add one workflow file to your
repo. Reviewers see the status check; nobody has to install
anything locally.

---

## Workflow file

Drop this into `.github/workflows/aegis.yml` of the project you
want Aegis to gate:

```yaml
name: Aegis Ring 0

on:
  pull_request:
    branches: [main]
    paths:
      - "**/*.py"

jobs:
  ring0:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Set up Python
        uses: actions/setup-python@v5
        with:
          python-version: "3.12"

      - name: Set up Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      # --- Build Aegis from source (V0.x — no PyPI yet) ---
      # When PyPI wheels ship, replace these three steps with:
      #   pip install aegis-control-plane
      - uses: actions/checkout@v4
        with:
          repository: wei9072/aegis
          path: _aegis

      - name: Install Aegis
        working-directory: _aegis
        run: |
          pip install maturin click google-genai prompt_toolkit
          cd aegis-core-rs && maturin build --release --out dist/
          pip install dist/*.whl

      # --- Run Ring 0 against the PR's changed Python files ---
      - name: Run Aegis check on PR diff
        run: |
          # Files changed in this PR (excluding deletions).
          files=$(git diff --name-only --diff-filter=ACM \
                    "${{ github.event.pull_request.base.sha }}" \
                    "${{ github.event.pull_request.head.sha }}" \
                  | grep '\.py$' || true)
          if [ -z "$files" ]; then
            echo "No Python changes — skipping."
            exit 0
          fi
          PYTHONPATH=_aegis python -m aegis.cli check $files
```

That's it. Push the workflow file, every subsequent PR gets a
"Aegis Ring 0" status check.

---

## What the PR review sees

Successful run:

```
✓ Aegis Ring 0 passed
   Checked 4 Python files; no Ring 0 violations.
```

Failed run (e.g. PR introduces a syntax error):

```
✗ Aegis Ring 0 failed
   src/foo.py:42: [Ring 0] Syntax error detected: expected ':' (line 42)
```

The PR's "checks" tab shows the full log. Reviewer can see the
verdict before approving.

---

## Pinning the Aegis version

The workflow above clones `wei9072/aegis@HEAD`. For reproducible
PR checks, pin a specific commit:

```yaml
      - uses: actions/checkout@v4
        with:
          repository: wei9072/aegis
          ref: 3efff25  # or a tag once we cut releases
          path: _aegis
```

Renovate / Dependabot can bump this like any other dependency.

---

## Caching the Rust build

The workflow as shown rebuilds the Rust extension on every PR.
Cache it for ~10× faster subsequent runs:

```yaml
      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            _aegis/aegis-core-rs/target
          key: ${{ runner.os }}-cargo-${{ hashFiles('_aegis/aegis-core-rs/Cargo.lock') }}
```

Insert this between "Set up Rust toolchain" and "Install Aegis".

---

## What Aegis doesn't gate (yet)

Same as the pre-commit hook:

- Ring 0 only (no cost-aware regression — needs HEAD-vs-PR-tip
  signal comparison; future work)
- No LLM-backed gates run in CI (those need API keys + would slow
  the PR check unacceptably)

For agents that iterate on code in CI (e.g., AI-assisted refactor
bots), the [MCP server](mcp_design.md) is the right surface — the
agent calls Aegis turn-by-turn during its run, not just at PR
submission.

---

## Required vs optional check

Mark the Aegis Ring 0 check **required** in your branch protection
rules to actually block merges. Without that, the check shows
red/green but doesn't prevent the merge. GitHub UI:

```
Settings → Branches → Branch protection rule → main →
  ✓ Require status checks to pass before merging →
    Add "Aegis Ring 0"
```

---

## Future cleanup

When PyPI wheels ship:

```yaml
      - run: pip install aegis-control-plane
      - run: |
          files=$(git diff --name-only --diff-filter=ACM ${{ github.event.pull_request.base.sha }} ${{ github.event.pull_request.head.sha }} | grep '\.py$' || true)
          [ -n "$files" ] && aegis check $files
```

10 lines instead of 30. The current verbose form is V0.x install
friction, not a permanent design choice.
