# GitHub Issue draft — "Rust build friction (V0.x onboarding)"

Copy the body below into a new GitHub issue. Suggested labels:
`onboarding`, `build-system`, `help-wanted`.

---

**Title:** Rust build friction — onboarding cliff for Python-only users

---

## Body

Aegis ships a Rust core (`aegis-core-rs/`) for fast structural-signal
extraction (fan-out, max-chain-depth, dependency cycles) via
tree-sitter + petgraph + PyO3. **This is currently the biggest
onboarding blocker for first-time Python users.**

### The current state (V0.x)

There is no `pyproject.toml` at the repo root. The README's
quickstart shows `pip install -e .`, but that doesn't actually work
yet — what works is:

```bash
cd aegis-core-rs
maturin develop --release   # requires venv active
# or, in CI:
maturin build --release --out dist/
pip install dist/*.whl
```

Plus you need Rust toolchain installed
(`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).

### What this issue tracks

We want to collect **specific friction reports** before deciding the
right fix. Possibilities, in roughly increasing investment:

1. **Documentation fix** — README current claim is misleading. Update
   to say `cd aegis-core-rs && maturin develop --release` until
   pyproject.toml lands.
2. **Add minimal `pyproject.toml`** at repo root using maturin as
   build backend. Then `pip install -e .` from the root would work.
   ~30 lines of config; the cleanest fix.
3. **Publish prebuilt wheels to PyPI** under `aegis-control-plane`.
   `pip install aegis-control-plane` would Just Work for the common
   platforms (Linux x86_64, macOS arm64, Windows x86_64). Requires
   PyPI account, version-publishing discipline, possibly cibuildwheel
   in CI.
4. **Provide a Docker image** for users who don't want to deal with
   Rust at all.

### Specific reports wanted

If you tried to install / use Aegis and got stuck on the build, please
share:

- **OS + Python version** (`python --version`, `uname -a`)
- **Did you have Rust installed already?** If no, was the rustup
  install obvious?
- **Which commands did you run?** Including the failures.
- **Where did you stop?** Did you give up, ask for help, find a
  workaround?

That last one matters most — we want to know which step actually
loses people.

### Acceptance criteria

This issue closes when one of:

- README + quickstart can be followed *without modification* by
  someone who's never built a Rust extension before
- OR `pip install aegis-control-plane` works on PyPI

Whichever comes first. Both could happen.

### Related

- [README quickstart](../../README.md#30-second-quickstart)
- [`aegis-core-rs/Cargo.toml`](../../aegis-core-rs/Cargo.toml)
- [`.github/workflows/test.yml`](../../.github/workflows/test.yml) —
  current CI build path (uses the `maturin build` + `pip install`
  workaround)
