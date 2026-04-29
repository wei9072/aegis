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
      - "**/*.ts"
      - "**/*.tsx"
      - "**/*.js"
      - "**/*.jsx"
      - "**/*.go"
      - "**/*.java"
      - "**/*.cs"
      - "**/*.php"
      - "**/*.swift"
      - "**/*.kt"
      - "**/*.dart"

jobs:
  ring0:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      # --- Install Aegis from a pinned source build ---
      # When V2.0 release artifacts ship, replace these with
      # `wget` of the pre-built linux-x86_64 tarball.
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/checkout@v4
        with:
          repository: wei9072/aegis
          path: _aegis
          ref: main          # pin to a commit SHA for reproducibility

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            _aegis/target
          key: ${{ runner.os }}-aegis-${{ hashFiles('_aegis/Cargo.lock') }}

      - name: Install aegis CLI
        working-directory: _aegis
        run: cargo install --path crates/aegis-cli --locked

      # --- Run Ring 0 against the PR's changed source files ---
      - name: Run Aegis check on PR diff
        run: |
          EXT_PATTERN='\.(py|pyi|ts|tsx|mts|cts|js|mjs|cjs|jsx|go|java|cs|php|phtml|swift|kt|kts|dart|rs)$'
          files=$(git diff --name-only --diff-filter=ACM \
                    "${{ github.event.pull_request.base.sha }}" \
                    "${{ github.event.pull_request.head.sha }}" \
                  | grep -E "$EXT_PATTERN" || true)
          if [ -z "$files" ]; then
            echo "No supported source changes — skipping."
            exit 0
          fi
          echo "$files" | xargs aegis check
```

That's it. Push the workflow file, every subsequent PR gets a
"Aegis Ring 0" status check.

---

## What the PR review sees

Successful run:

```
✓ Aegis Ring 0 passed
   Checked 4 source files; no Ring 0 violations.
```

Failed run (PR introduces a syntax error):

```
✗ Aegis Ring 0 failed
   src/foo.py:42: [Ring 0] Syntax error detected: expected ':' (line 42)
```

The PR's "checks" tab shows the full log. Reviewer can see the
verdict before approving.

---

## Pinning the Aegis version

The workflow above clones `wei9072/aegis@main`. For reproducible
PR checks, pin to a commit SHA or a release tag:

```yaml
      - uses: actions/checkout@v4
        with:
          repository: wei9072/aegis
          ref: v0.1.0        # or a commit SHA
          path: _aegis
```

Renovate / Dependabot can bump this like any other dependency.

---

## What Aegis doesn't gate (yet)

Same as the pre-commit hook:

- Ring 0 only (no cost-aware regression — would need a
  base-vs-head signal comparison; tracked as backlog in
  [`ROADMAP.md`](../ROADMAP.md))
- No LLM-backed gates run in CI (those need API keys + would slow
  the PR check unacceptably)

For agents that iterate on code in CI (e.g. AI-assisted refactor
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

## Future cleanup (post-V2.0 release)

Once `aegis` ships as a pre-built binary on GitHub Releases, the
install steps shrink to ~3 lines:

```yaml
      - name: Install aegis
        run: |
          wget -q -O- https://github.com/wei9072/aegis/releases/latest/download/aegis-x86_64-unknown-linux-gnu.tar.gz | tar xz
          sudo mv aegis /usr/local/bin/
```

Drop the cargo cache, the `dtolnay/rust-toolchain` setup, and the
checkout of the `_aegis` repo entirely. The current verbose form
is V1.10 install friction, not a permanent design choice.
