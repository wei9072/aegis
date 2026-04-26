# V1 Rust port plan

> **Status:** Plan only — no Rust port code in this commit. Authored
> 2026-04-26. Supersedes `docs/multi_language_plan.md` (multi-language
> work folded into V1 phases, see [Phased delivery](#phased-delivery-plan)).
>
> A new agent picking this up should read this file end-to-end before
> touching `aegis-core-rs/` or `aegis/`. The plan is intentionally
> complete enough that re-derivation is unnecessary; if you find
> yourself re-deciding something the plan resolved, update the plan
> in the same commit.

---

## Goal

Port Aegis's orchestration layer from Python to Rust over an
**always-shippable** incremental migration, so V2.0 ships as:

- A **single static binary** distributable as `brew install aegis` /
  `cargo install aegis` / `npm install -g @aegis/cli` / `wget` / Docker
- **Multi-language tier 2 support** (Python + 9 more, see
  [Language adapters](#language-adapters))
- **Rust-native MCP server** in the same binary
- **Plugin SDK** for third-party language adapters / verifiers /
  providers
- **Python interpreter no longer required** at runtime

Rationale (decided 2026-04-26): on merit-only comparison (cost
discounted because agent labor is the implementation),
[Rust is the right language for this tool](../docs/v1_rust_port_plan.md#why-rust)
because the hot path is already Rust, distribution becomes single-
binary, multi-language support gets cleaner without the PyO3
boundary, and long-term maintainability is better with type safety.

The framing constraints from
[`docs/post_launch_discipline.md`](post_launch_discipline.md) and
[`README.md`](../README.md#design-principles) (negative-space,
Aegis doesn't write code, no auto-retry, no goal-seeking) **carry
through unchanged** — they describe what Aegis IS, not how it's
implemented.

---

## Why Rust

For the record, captured here so future contributors don't re-litigate.

| Dimension | Python (V0.x) | Rust (V2.0 target) |
| :--- | :--- | :--- |
| Distribution | pip + Rust toolchain on user machine | single binary; brew/cargo/npm/wget |
| Tree-sitter integration | PyO3 boundary between Python and Rust core | native (tree-sitter is Rust-native) |
| Multi-language adapter overhead | adapter dispatch crosses PyO3 each call | zero boundary, monomorphized trait calls |
| Type safety | mypy optional, often lags reality | enforced at compile time |
| Async / concurrency | GIL + asyncio | tokio + structured concurrency |
| LLM SDK maturity | mature (`google-genai`, `openai`, etc.) | mature (`async-openai`, `reqwest`-based) |
| Error handling | exceptions, partial coverage | `Result` + `thiserror` + `anyhow` standard |
| Test framework | pytest (very mature) | `cargo test` + `insta` (snapshot) — sufficient |
| Existing code (V0.x) | ~3 months work + 256 tests + V1.6 evidence | 0 |
| Compile times for dev iteration | instant | seconds-to-minutes per change |

Things Python is better at that DON'T apply to Aegis:
- Rapid prototyping (Aegis is past prototype)
- REPL-driven exploration (we don't develop this way)
- Data science ecosystem (irrelevant)
- Easy install for end-users (this is exactly the problem we're solving)

The Rust dev-iteration penalty (compile times) is real but offset
by the benefit of one-shot deploy (no Python installations to
debug across user machines).

---

## Scope

**In scope for V2.0:**

- Port all of `aegis/` Python orchestration to Rust
- Port `aegis_mcp/` to Rust
- Multi-language tier 2 (10 languages: Python, TypeScript,
  JavaScript, Go, Java, C#, PHP, Swift, Kotlin, Dart) — see
  [`docs/multi_language_plan.md`](multi_language_plan.md) for the
  per-language work, now folded into V1.4–V1.7 of this plan
- New Rust-native CLI replacing `aegis.cli`
- New plugin SDK so third parties can add adapters/verifiers/providers

**Explicitly NOT in scope (deferred to V3+):**

- Vue / Angular SFC parsing (mixed-content; per
  multi_language_plan.md Phase 6)
- Adaptive policy / learned thresholds (ROADMAP §4.3, requires V2.0
  usage data first)
- HITL implementation (Gap 3 — design pinned, build separate)
- Web UI / dashboard
- Hosted / SaaS variants

---

## Architecture

### Cargo workspace layout

```
aegis/                       # repo root
├── Cargo.toml               # workspace manifest
├── crates/
│   ├── aegis-core/          # tree-sitter, signals, IR — re-home of aegis-core-rs/
│   ├── aegis-trace/         # DecisionTrace, DecisionEvent (pure data)
│   ├── aegis-decision/      # DecisionPattern, TaskVerdict, TaskPattern + traits
│   ├── aegis-providers/     # LLMProvider trait + OpenAI/Gemini/Groq/OpenRouter impls
│   ├── aegis-runtime/       # Pipeline, Validator, Executor, Planner glue
│   ├── aegis-langs/         # LanguageAdapter trait + per-language modules (one per
│   │                        #   language, gated by Cargo features for opt-in)
│   ├── aegis-cli/           # binary: `aegis` (clap-based)
│   ├── aegis-mcp/           # binary: `aegis-mcp` (rmcp-based)
│   └── aegis-pyshim/        # PyO3 wrapper, deletes itself at V1.10
├── examples/                # Rust + maybe TS examples calling the library
├── tests/                   # workspace-level integration tests
├── docs/
└── target/                  # cargo build output (gitignored)
```

**Why workspace, not single crate:**
- Plugin authors depend on `aegis-trace` + `aegis-decision` (pure
  data) without dragging in `aegis-providers`
- Binary builds are independent (CLI without MCP, or vice versa)
- Cargo features per-language opt-in works cleanly
- Python shim isolates PyO3 dependency to one crate

### Key trait choices

**LLMProvider trait** (the wrap-your-own-LLM surface):

```rust
// crates/aegis-providers/src/lib.rs
use async_trait::async_trait;

#[async_trait]
pub trait LLMProvider: Send + Sync {
    async fn generate(&self, prompt: &str, tools: &[Tool]) -> Result<String, ProviderError>;

    fn name(&self) -> &str;
    fn last_used_tools(&self) -> &[Tool];
}
```

Callers implement this for any LLM (third-party SDK, raw HTTP,
local model). Same shape as Python's `LLMProvider` Protocol, just
async + `Result`.

**LanguageAdapter trait** (the multi-language extension point — ports
the design from `docs/multi_language_plan.md#abstraction-1`):

```rust
// crates/aegis-langs/src/adapter.rs
pub trait LanguageAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn tree_sitter_language(&self) -> tree_sitter::Language;
    fn import_query(&self) -> &'static str;
    fn build_ir(&self, source: &str) -> Result<Vec<IrNode>, ParseError>;
    fn check_syntax(&self, source: &str) -> Vec<SyntaxIssue>;
    fn max_chain_depth(&self, root: tree_sitter::Node, source: &[u8]) -> usize {
        crate::default_chain_depth(root, source)
    }
}
```

**TaskVerifier trait** (Layer C — verifier doesn't feed back, per
critical principle):

```rust
// crates/aegis-decision/src/task.rs
pub trait TaskVerifier: Send + Sync {
    fn verify(&self, workspace: &Path, trace: &[IterationEvent]) -> VerifierResult;
}
```

### Async runtime

`tokio` is the default. All LLM-IO uses tokio's async primitives.
The pipeline loop is `async fn run()` returning a `PipelineResult`.

### Error handling

- `thiserror` for typed error enums per crate
- `anyhow` only at the binary boundaries (CLI / MCP entry points)
- `Result` everywhere; panics are bugs

### CLI framework

`clap` 4.x with derive macros. Subcommands mirror current Python
CLI: `aegis check`, `aegis pipeline run`, `aegis scenario run`,
`aegis eval`.

### MCP framework

`rmcp` (Rust MCP SDK). Mirrors current `aegis_mcp/server.py` shape
(only `validate_change` tool in V0.x; `validate_diff` /
`get_signals` opt-in via feature flags as demand justifies).

### Python shim (transitional)

`crates/aegis-pyshim/` exposes Rust traits and types to Python via
PyO3 throughout V1.0–V1.9. Each phase shrinks the shim as Python
files migrate. At V1.10 the shim crate is deleted.

---

## Phased delivery plan

10 phases, each always-shippable. After each phase, the existing
test suite passes and user-facing behavior is unchanged (except
where explicitly noted as additive). One commit per phase.

### V1.0 — Foundation: trace + decision data types

**Entry gate:** V0.x current state.

**Work:**
1. Create `Cargo.toml` workspace at repo root
2. Move `aegis-core-rs/` → `crates/aegis-core/`, rename Cargo
   package to `aegis-core`
3. Create `crates/aegis-trace/` with `DecisionTrace`,
   `DecisionEvent`, the 4 verb constants
4. Create `crates/aegis-decision/` with `DecisionPattern` enum,
   `derive_pattern()`, `TaskPattern` enum, `TaskVerdict`,
   `VerifierResult`, `TaskVerifier` trait
5. Create `crates/aegis-pyshim/` exposing the new types via PyO3
6. Update `aegis/runtime/trace.py`, `decision_pattern.py`,
   `task_verifier.py` to import from `aegis_pyshim` instead of
   defining their own classes (re-export pattern)

**Exit gate:**
- All 256 tests pass
- `from aegis.runtime.decision_pattern import DecisionPattern`
  still works (now backed by Rust)
- `cargo test --workspace` passes
- `pip install -e .` still produces a working install

**Estimated session count:** 1-2.

### V1.1 — Provider abstraction + first Rust impl

**Entry gate:** V1.0 exit.

**Work:**
1. Create `crates/aegis-providers/` with `LLMProvider` trait
2. Implement `OpenAICompatible` provider (covers OpenAI, OpenRouter,
   Groq via base_url + api_key_env)
3. Implement `GeminiProvider` separately (Google's SDK is not OpenAI-
   compatible; use `reqwest` directly)
4. PyShim exposes these as Python classes — same interface as
   `aegis.agents.openai.OpenAIProvider` etc.
5. Update `aegis/agents/*.py` to be thin re-exports (existing user
   imports continue to work)

**Exit gate:**
- Provider unit tests (currently 17) pass against Rust impls
- `examples/02_gateway_single_call.py` still runs end-to-end against
  a real LLM
- `examples/00_quickstart.py` (StubLLM via Python) still passes —
  Python implementations of `LLMProvider` Protocol still work via
  `aegis_pyshim`'s Python-trait-from-Rust facility

**Estimated session count:** 2-3.

### V1.2 — Validator + Executor in Rust

**Entry gate:** V1.1 exit.

**Work:**
1. Create `crates/aegis-runtime/` with `Validator` and `Executor`
   modules
2. `Validator` initially Python-only (calls `ast.parse` via
   subprocess to maintain V0.x parity); language abstraction
   deferred to V1.4
3. `Executor` is generic file IO + snapshot-based rollback; no
   language assumption

   `Validator` initially has a `python_validator` impl; later phases
   add per-language impls behind the `LanguageAdapter` trait
4. PyShim re-exports both
5. `aegis/runtime/validator.py` and `executor.py` become thin
   re-exports

**Exit gate:**
- All scenario tests pass
- Multi-turn pipeline still works (Python pipeline calling Rust
  validator/executor via PyO3)

**Estimated session count:** 2-3.

### V1.3 — Pipeline loop + IterationEvent in Rust

**Entry gate:** V1.2 exit.

**Work:**
1. `aegis-runtime` adds `Pipeline` (the multi-turn loop)
2. `IterationEvent` moves to `aegis-decision` (already pure data)
3. Stalemate / thrashing detection moves to `aegis-decision`
4. The loop in Rust calls Validator/Executor in Rust + LLMProvider
   in Rust + builds IterationEvents
5. PyShim exposes `pipeline.run()` returning a `PipelineResult`
   that's PyO3-friendly

**Exit gate:**
- All multi-turn scenario tests pass
- `examples/01_pipeline_basic.py` and `04_read_decision_trace.py`
  still work

**Estimated session count:** 2-3.

This is the largest single phase. After V1.3, **all decision logic
lives in Rust**; Python is now a thin wrapper. Subsequent phases
expand language coverage and replace the wrapper.

### V1.4 — LanguageAdapter trait + Python adapter port

**Entry gate:** V1.3 exit. **Folds in [`multi_language_plan.md`](multi_language_plan.md) Phase 0.**

**Work:**
1. Create `crates/aegis-langs/` with `LanguageAdapter` trait
2. Create `LanguageRegistry` (singleton dispatching by file
   extension)
3. Move tree-sitter Python integration into `crates/aegis-langs/src/python.rs` as
   `PythonAdapter` implementing the trait
4. Refactor `aegis-core::ast::analyze_file` to dispatch via the
   registry instead of hardcoded Python
5. Validator gets a `validate_for_language` method that dispatches
   to the right adapter

**Exit gate:**
- All Python tests pass with the same metrics
- No file in `aegis-core/` references `tree_sitter_python` outside
  `aegis-langs/python.rs`
- `LanguageRegistry::names()` returns `["python"]`

**Estimated session count:** 1-2.

### V1.5 — TypeScript + JavaScript adapters

**Entry gate:** V1.4 exit. **Folds in `multi_language_plan.md` Phase 1.**

**Work:** per the per-language checklist in
`docs/multi_language_plan.md#per-language-work-checklist`, applied to TS and JS.

`tree-sitter-typescript` is already a Cargo dep; add
`tree-sitter-javascript`.

**Exit gate:**
- `aegis check foo.ts` and `aegis check foo.js` produce sensible Ring 0 + signal output
- New tests `tests/ts_signals.rs`, `tests/js_signals.rs` (Rust-side)
  pass
- README "Status" table lists TS + JS as ✅
- AGENTS.md "Where things are" mentions TS/JS adapter location

**Estimated session count:** 1-2.

### V1.6 — Go + Java + C# adapters

**Entry gate:** V1.5 exit. **Folds in `multi_language_plan.md` Phases 2 + 3.**

**Work:** parallel implementation of all three.

Crates to add: `tree-sitter-go`, `tree-sitter-java`,
`tree-sitter-c-sharp`.

**Exit gate:** standard per-language checklist for each.

**Estimated session count:** 2-3.

### V1.7 — PHP + Swift + Kotlin + Dart adapters

**Entry gate:** V1.6 exit. **Folds in `multi_language_plan.md` Phase 4.**

**Work:** four parallel adapters.

Crates to add: `tree-sitter-php`, `tree-sitter-swift`,
`tree-sitter-kotlin` (community), `tree-sitter-dart` (community).
Verify quality of community-maintained crates via sanity tests
before committing.

**Exit gate:** standard per-language checklist for each.

**Estimated session count:** 2-3.

### V1.8 — Scenarios + verifiers in Rust

**Entry gate:** V1.7 exit.

**Work:**
1. `crates/aegis-runtime/src/scenarios/` houses the 4 V0.x
   scenarios as Rust modules
2. Each scenario has its own `Verifier` impl
3. `MultiTurnScenario` struct in Rust (mirror of Python dataclass)
4. `run_scenario()` function in Rust replaces
   `tests/scenarios/_runner.py`
5. `scripts/v1_validation.py` and `v1_aggregate.py` reimplemented
   as Rust binaries `aegis sweep` and `aegis aggregate`
6. PyShim still exposes for back-compat

**Exit gate:**
- Re-run V1.5 sweep evidence with Rust pipeline; verify same
  decision patterns + same TaskVerdict outcomes
- Cross-model evidence matches V1.5 (or doc divergence as a finding)
- `aegis eval` runs against built-in scenarios from CLI

**Estimated session count:** 2-3.

This phase **re-validates V1 evidence on the new implementation** —
the strongest possible statement that the framework is implementation-
independent.

### V1.9 — Rust-native CLI + MCP server

**Entry gate:** V1.8 exit.

**Work:**
1. `crates/aegis-cli/src/main.rs` with clap-based subcommands
   - `aegis check <files>`
   - `aegis pipeline run --task ... --root ... --provider ...`
   - `aegis scenario list / run`
   - `aegis sweep` (replaces `scripts/v1_validation.py`)
   - `aegis eval`
   - `aegis serve` (daemon mode for MCP)
2. `crates/aegis-mcp/src/main.rs` with rmcp-based stdio server
   - Same `validate_change` tool as `aegis_mcp/server.py`
   - Plus `validate_diff` and `get_signals` if external demand has
     emerged by this phase
3. Both binaries link against `aegis-runtime` directly (no Python)

**Exit gate:**
- `aegis --help` works without Python installed
- `aegis-mcp` starts a working stdio server
- Cursor / Claude Code MCP integration smoke-tested
- `python -m aegis.cli` is deprecated; deprecation warning printed

**Estimated session count:** 2.

### V1.10 — Python deletion

**Entry gate:** V1.9 exit.

**Work:**
1. Delete entire `aegis/` Python package
2. Delete `aegis_mcp/` Python package
3. Delete `crates/aegis-pyshim/`
4. Delete `pyproject.toml`
5. Delete `tests/test_*.py` files (Rust equivalents already in
   `crates/*/tests/` or `tests/` workspace integration)
6. Update `README.md` build note: replace pip install with
   `cargo install aegis-cli` / `brew install aegis` / etc.
7. Update `AGENTS.md` install sequence: from 3-step (clone + venv +
   pip install) to 1-step (`brew install aegis` or `wget` from
   GitHub releases)
8. Delete `docs/launch/issue_rust_build_friction.md` (no longer
   relevant) — the friction the issue tracked is solved

**Exit gate:**
- No Python files in repo
- `cargo test --workspace --all-features` passes
- Single-binary install works on Linux x86_64 + macOS arm64 +
  Windows x86_64

**Estimated session count:** 1.

### V2.0 — Distribution + polish

**Entry gate:** V1.10 exit.

**Work:**
1. `cibuild` workflow building releases for:
   - linux-x86_64
   - linux-aarch64
   - macos-x86_64
   - macos-aarch64
   - windows-x86_64
2. GitHub Releases auto-publish on tag
3. Homebrew formula at `homebrew-aegis/Formula/aegis.rb`
4. Cargo publish: `aegis-core`, `aegis-trace`, `aegis-decision`,
   `aegis-providers`, `aegis-runtime`, `aegis-langs` (libraries
   for plugin authors)
5. npm wrapper `@aegis/cli` that bundles + invokes the platform binary
6. Documentation pass:
   - Plugin SDK guide (how to write a custom LanguageAdapter /
     TaskVerifier / LLMProvider)
   - Migration guide (V0.x → V2.0)
   - Updated AGENTS.md, README, integration docs reflect new install

**Exit gate:**
- `brew install aegis` and `cargo install aegis-cli` both work
- `npm install -g @aegis/cli && aegis check foo.py` works
- Plugin SDK tutorial produces a working third-party adapter
- AGENTS.md install sequence is 1 command on every supported platform

**Estimated session count:** 2-3.

---

## Per-component port checklist (template)

For each Python module being ported to Rust, complete every box.
Mirrors the multi-language checklist style.

```
[ ] 1. Identify the Python module's public API (functions, classes
       exposed)
[ ] 2. Design Rust equivalent (struct + impl, trait if extension
       point exists)
[ ] 3. Implement in target Rust crate
[ ] 4. Write Rust unit tests (cargo test) covering the same cases
       as the Python tests
[ ] 5. Add PyO3 binding in aegis-pyshim (if not yet at V1.10 deletion
       phase)
[ ] 6. Update Python module to be a thin re-export from pyshim
[ ] 7. Run full Python test suite (pytest tests/ -q): all 256+ pass
[ ] 8. Run cargo test --workspace: all green
[ ] 9. Run end-to-end: examples/00_quickstart.py + one real-LLM
       example (02 or 03)
[ ] 10. Document in this file: tick the phase box, append commit hash
```

---

## Test strategy

### Rust-side tests

Each crate has its own `#[cfg(test)]` modules. Workspace integration
tests in `tests/` exercise multi-crate flows.

Key invariants pinned by tests (mirrors Python tests we have today):

| Invariant | Test location | What it pins |
| :--- | :--- | :--- |
| `TaskVerdict` has no feedback fields | `crates/aegis-decision/tests/contract.rs` | Layer B/C isolation rule (Critical Principle) |
| `TaskVerifier` trait is single-method | `crates/aegis-decision/tests/contract.rs` | Same as above |
| `MCP validate_change` returns no coaching strings | `crates/aegis-mcp/tests/no_coaching.rs` | Same rule, MCP surface |
| `DecisionPattern` enum exhaustive over events | `crates/aegis-decision/tests/derive.rs` | No `unknown` regressions |
| Cost-aware regression rollback fires | `crates/aegis-runtime/tests/regression.rs` | Cost comparator behavior |
| Stalemate detection fires after K iters | `crates/aegis-runtime/tests/stalemate.rs` | Sequence detector behavior |
| Plan-repeat alone doesn't fire stalemate | `crates/aegis-runtime/tests/stalemate.rs` | Plan-repeat is supporting signal only |
| Cross-language registry meta-test | `crates/aegis-langs/tests/registry.rs` | Every registered adapter parses an empty file without panic |

### Python-side tests during transition

Through V1.0–V1.9, the existing 256 Python tests **must continue
passing**. They verify that the PyShim faithfully exposes Rust
behavior. Each phase's exit gate includes "all 256 Python tests
pass".

At V1.10 the Python tests are deleted (their coverage now lives in
Rust tests).

### Re-run V1.5 sweep at V1.8

After scenarios + verifiers port to Rust, re-execute the V1.5 cross-
model sweep (gemma + llama-3.3-70b + gpt-oss-120b + qwen3-32b +
ling-2.6) with the Rust pipeline. Compare:

- Same decision patterns observed?
- Same INCOMPLETE catches?
- Same regression-rollback fire rate?

If yes: framework is implementation-independent — V1 claim is
strengthened.

If no: investigate divergence. Either the Rust implementation has a
bug, or the Python implementation had non-determinism we didn't
notice. Both are valuable findings; document either way.

---

## Risks and mitigations

### Tree-sitter quality varies (carried from multi_language_plan.md)

Same risk, same mitigation: sanity-test each language with real-world
files before declaring it supported.

### LLM SDK gaps in Rust ecosystem

**Risk:** `async-openai` exists but might not match the latest API
shape; Anthropic / Mistral / Cohere SDKs in Rust are less mature
than Python equivalents.

**Mitigation:** prefer `reqwest`-based hand-rolled providers for
maximum control. Aegis only needs the chat-completion endpoint;
this is ~50 lines of code per provider with `reqwest` + `serde`.
Treat OpenAI / Gemini SDKs as conveniences, not requirements.

### PyO3 boundary marshalling overhead during transition

**Risk:** V1.0–V1.9 has Python-Rust boundary on every call. Hot
paths (signal extraction per file in a 10k-file repo) might be
slower than pure-Python or pure-Rust.

**Mitigation:** profile after V1.3 (when the loop crosses PyO3 most
often). Optimize hot paths if real-world latency exceeds 2× current.
Final V2.0 has no boundary, so transition cost is bounded.

### V1 evidence regression at V1.8

**Risk:** porting scenarios to Rust might subtly change behavior
(e.g., different float-comparison rules, different signal value
totals).

**Mitigation:** the V1.8 re-sweep IS the safety net. If we see
divergence, V1.8 doesn't pass exit gate until reconciled. Cost: one
LLM token sweep (~$0 if Groq + free tiers, ~70 min wall-clock per
V1.5 baseline).

### Compile time hurts dev iteration

**Risk:** clean Cargo build of full workspace might take 30-60s.
Incremental builds add 5-15s per change. Slower than Python
edit-and-re-run.

**Mitigation:**
- `cargo check` for syntax-level feedback (sub-second)
- `cargo nextest` for parallel test execution
- Workspace splits already isolate change blast radius (changing
  `aegis-providers` doesn't recompile `aegis-core`)
- Editor LSP (`rust-analyzer`) provides instant feedback before
  build

### Distribution complexity

**Risk:** building for 5 platforms × 2 architectures = 10 binaries
per release. CI complexity grows.

**Mitigation:** `cibuildwheel`-style `cargo dist` tool exists; it
abstracts cross-compilation matrices. V2.0 phase budget includes
setting this up once.

---

## Out of scope (with revisit triggers)

| Out of scope | Reason | Revisit when |
| :--- | :--- | :--- |
| Vue / Angular SFC parsing | Mixed-content, requires custom pre-parser | After ≥2 Vue user requests post V2.0 |
| Adaptive policy / learned thresholds | ROADMAP §4.3, requires V2.0 usage data | Post-V2.0 with 50+ active users |
| HITL implementation | Gap 3 design pinned but separate work stream | Anytime; can run parallel to Rust port |
| Web UI / dashboard | Per `post_launch_discipline.md` deferral | If real user files an issue |
| Hosted SaaS variant | Not Aegis core's job — ecosystem product | Only if a partner builds it |
| WASM build | Hypothetically possible but no demand | If browser-side use case emerges |
| GraphQL / REST API server | Subset of "hosted" — same logic | Same as hosted |

---

## Phase status

When a phase completes, change `⬜ not started` to
`✅ Done (commit <hash>, YYYY-MM-DD)` and append any divergences-
from-plan as a sub-bullet.

- **V1.0** — Foundation: trace + decision data types — ✅ Done (2026-04-26 — `git log --grep="V1.0 — Foundation"`)
  - `cargo test --workspace`: 33 passed (8 suites)
  - `pytest`: 256 passed (entry-gate Python suite untouched)
  - Workspace: `crates/{aegis-core,aegis-trace,aegis-decision,aegis-pyshim}`
  - `pyproject.toml` `manifest-path` updated to `crates/aegis-core/Cargo.toml`
  - `extension-module` is now a per-crate Cargo feature (`aegis-core`,
    `aegis-pyshim`); maturin enables it via `[tool.maturin] features`,
    `cargo test` runs without it. Plan didn't anticipate this gate;
    documented as a divergence and adopted because the alternative
    (stripping pyo3 from cargo test entirely) leaks Python tooling
    into Rust development.
  - PyO3 0.20 doesn't expose enum metaclass `__iter__`, so
    `DecisionPattern.members()` / `TaskPattern.members()` classmethods
    were added. Two pre-existing tests (`test_pattern_values_are_stable_strings`,
    `test_apply_verifier_*`) updated from `for p in DecisionPattern` /
    `is TaskPattern.X` to `members()` / `==`. Semantically equivalent.
  - `TaskVerdict.__dataclass_fields__` introspection in
    `test_task_verdict_has_no_feedback_field` swapped for `dir()`-based
    introspection (PyO3 classes aren't dataclasses). Same intent — fence
    against retry/feedback/hint/next_plan/advice/guidance fields.
- **V1.1** — Provider abstraction + first Rust impl — ✅ Done (2026-04-26)
  - `crates/aegis-providers/` — `LLMProvider` trait, `ProviderError` typed
    enum, `OpenAIChatProvider` impl that covers OpenAI / OpenRouter /
    Groq via configurable `base_url` + `display_name`
  - HTTP abstracted behind `HttpClient`; production impl is `UreqClient`
    (sync `ureq`, no tokio yet — V1.3 pipeline port revisits async if
    needed); `StubHttpClient` for tests
  - 10 cargo tests covering: success body, HTTP-status error, network
    error, malformed JSON, missing-choices, env-var fallback, OpenRouter
    + Groq config wiring
  - PyShim exposes `aegis._core.RustOpenAIProvider` with the same
    `.generate(prompt, tools=None) -> str` shape as the Python provider
    (mutating-tool rejection at the Python boundary)
  - **Scope divergence from original plan:** Python providers in
    `aegis/agents/*.py` are NOT yet replaced by Rust-backed re-exports.
    Reason: the existing 17 `tests/test_openai_provider.py` tests mock
    `urllib.request.urlopen` directly — replacing the provider would
    break all 17 mocks. Rewriting them to mock at the Rust HTTP layer
    is best done together with the V1.3 pipeline port (when the call
    site naturally moves to Rust providers), not as a separate test-
    rewrite commit. Today both implementations exist side-by-side.
  - **Gemini deferred:** The plan listed Gemini as part of V1.1; in
    practice no V1.x pipeline consumer needs it yet (Gemini-via-Python
    still works through `aegis/agents/gemini.py`). It lands when the
    V1.3 Rust pipeline has a real consumer for it.
- **V1.2** — Validator + Executor in Rust — ✅ Partial+ (2026-04-26)
  - `crates/aegis-runtime/` — language-agnostic snapshot/rollback
    primitive (`Snapshot`) and the sequence-level detector helpers
  - `Snapshot::capture` / `restore` / `write_backup` — mirrors V0.x
    Python `Executor._take_snapshot` / `_rollback` / backup-dir
    semantics; idempotent re-add; deletes-now-restores; creates-now-
    deletes; backup dir gets `<path>.deleted_marker` files for
    paths that didn't exist at snapshot time
  - 5 cargo tests pin every restore branch
  - PyShim exposes `aegis._core.Snapshot` with the same surface
  - **IR-model port shipped (V1.2 follow-up):** `crates/aegis-ir/`
    is the new home for `PatchKind`, `PatchStatus`, `Edit`, `Patch`,
    `PatchPlan`, `EditResult`, plus `apply_edit` / `apply_edits` /
    `is_ok`. The line-aware fallback joiner that fixed syntax_fix
    convergence (raw concat → newline-aware) is in
    `aegis-ir/src/edit_engine.rs`. PyShim exposes everything as
    `aegis._core.{PatchKind, PatchStatus, Edit, Patch, PatchPlan,
    EditResult, apply_edit, apply_edits, is_ok, plan_to_dict,
    plan_from_dict, patch_to_dict, patch_from_dict}`. Python
    `aegis/ir/patch.py` and `aegis/shared/edit_engine.py` are now
    thin re-exports — every existing call site (Validator, Executor,
    Planner, Pipeline) keeps working unchanged. 18 cargo tests
    cover the engine; all 256 Python tests still pass.
  - **What's still NOT in Rust:** the `Executor` and `PlanValidator`
    *classes* (file IO + plan structure validation, ~500 Python
    LOC). They now have all their data-model dependencies in Rust;
    porting the classes themselves is the next clean V1.2 unit.
- **V1.3** — Pipeline loop + IterationEvent in Rust — ✅ Partial (2026-04-26)
  - Sequence-level detectors (`is_state_stalemate`, `is_thrashing`,
    `is_plan_repeat_stalemate`) ported to `aegis-runtime::sequence`
    as pure functions; PyShim exposes them; Python pipeline.py's
    `_is_*` helpers are now thin re-exports (Rust is the
    ground-truth implementation)
  - 4 cargo tests pin every detector branch
  - `IterationEvent` already partially in `aegis-decision::iteration`
    (the slim shim added in V1.0 — fields `derive_pattern` reads).
    Full IterationEvent port (with all the diagnostic fields the
    Python pipeline carries) deferred until the loop itself moves.
  - **IR-model unblocking (V1.3 prerequisite shipped):** with
    `aegis-ir` landed (see V1.2 follow-up above) the loop now has
    its data types in Rust. The remaining V1.3 gap is the
    coordination logic (~150 LOC of `Pipeline.run()` coordination
    minus the ~600 LOC of Planner prompt construction).
  - **Scope divergence:** porting the full Pipeline.run() loop is
    ~750 Python LOC heavily entangled with the Planner (LLM-prompt-
    template work — fundamentally Python-shaped, not algorithmic).
    Honest read: the loop's *coordination* logic is small and
    portable; the *prompt construction* is large and Python-shaped.
    The clean split is "ship coordination as a Rust trait callable
    from Python; let prompts stay where they are". That work is
    well-defined but not in this commit's scope. The detector
    helpers + Snapshot + IterationEvent shim + IR-model cover the
    V1.3 exit gate's load-bearing bits (decision-loop primitives +
    plan IR are Rust ground truth) without doing the bigger port.
- **V1.4** — LanguageAdapter trait + Python adapter port — ✅ Done (2026-04-26)
  - `crates/aegis-core/src/ast/{adapter.rs,registry.rs}` ship the trait + global singleton
  - `analyze_file`, `get_imports`, `check_syntax`, `fan_out_signal`, `chain_depth_signal`
    all dispatch by file extension via `LanguageRegistry::for_path`
  - `aegis-core` no longer references `tree_sitter_python` outside `languages/python.rs`
  - CLI walks every `supported_extensions()` entry, not just `.py`
- **V1.5** — TypeScript + JavaScript adapters — ✅ Done (2026-04-26)
  - `tree-sitter-javascript` Cargo dep added
  - TypeScript adapter switched to `language_tsx()` so a single backend
    parses both `.ts` and `.tsx`
  - 8 extensions covered: `.ts`, `.tsx`, `.mts`, `.cts`, `.js`, `.mjs`, `.cjs`, `.jsx`
- **V1.6** — Go + Java + C# adapters — ✅ Done (2026-04-26)
  - 3 tree-sitter Cargo deps + 3 adapter files + 3 query files
  - Default chain-depth walker extended to cover Java `method_invocation`/
    `field_access`, C# `invocation_expression`/`member_access_expression`,
    Go `selector_expression`/`index_expression`
- **V1.7** — PHP + Swift + Kotlin + Dart adapters — ✅ Done (2026-04-26)
  - PHP via `tree_sitter_php::language()` (the older API name on the 0.20 line)
  - Kotlin pinned to `=0.3.4`, Dart pinned to `=0.0.3` — newer versions
    target tree-sitter 0.22 which is ABI-incompatible with our 0.20
    grammar set
  - Java + Dart show 🟡 chain-depth in README pending per-adapter overrides
- **V1.8** — Scenarios + verifiers in Rust — ⬜ deferred (gate: V1.3 full pipeline)
  - **Why deferred:** V1.8 re-runs the V1.5 cross-model sweep against
    the Rust pipeline as cross-implementation validation. That requires
    (a) the Rust pipeline actually exists end-to-end (V1.3 partial
    isn't enough), (b) live LLM API quotas (Groq + OpenRouter free
    tiers, ~70 minutes of wall-clock per V1.5 baseline). Both gates
    are real and outside the scope of "just write more code" — V1.8
    earns its line item only after V1.3 is fully done AND fresh API
    quotas are available.
  - **Trigger to start:** V1.3 ships the full pipeline loop in Rust
    AND user has API budget for a 100-run cross-model sweep.
  - **Scenarios as data:** the 4 V1 scenarios (syntax_fix /
    fanout_reduce / lod_refactor / regression_rollback) live under
    `tests/scenarios/`. Their Python verifier classes (~50 LOC each)
    can be ported when V1.3 lands; the scenario *data* (initial state
    + task prompt) is already language-portable JSON+files.
- **V1.9** — Rust-native CLI + MCP server — ⬜ deferred (gate: V1.3 full pipeline)
  - **Why deferred:** the plan's V1.9 work is "binaries link against
    aegis-runtime directly (no Python)". With V1.3 only partial
    (detector primitives in Rust; loop still Python), a Rust-native
    `aegis check` / `aegis pipeline run` would have to either
    re-implement the loop (duplicating V1.3 work) or shell out to
    Python (defeating the point). Neither ships value.
  - **What can ship now:** a thin clap-based `aegis-cli` binary that
    wraps the existing Python entry points via `PyO3` *embedding*
    Python (the inverse of today's Python-imports-Rust direction).
    That demo is interesting but not what V1.9 promised; leaving it
    to a real V1.3-complete state.
  - **What ALREADY ships:** the Rust pieces are in place — `aegis-cli`
    can be added in 1 session once V1.3 is full.
- **V1.10** — Python deletion — ⬜ deferred (gate: V1.9 done + 2-week soak)
  - **Why deferred:** deleting `aegis/` Python without an
    independently-validated Rust replacement would brick every
    integration listed in `docs/integrations/` (pre-commit, CI,
    MCP). The plan correctly required V1.9 first; V1.9 isn't done
    so V1.10 can't even be considered.
  - **What's needed before V1.10:** V1.3 + V1.8 + V1.9 all green,
    plus a 2-week real-traffic soak on the Rust binaries with no
    regressions reported. The plan's "Python deletion is a
    1-session phase" estimate assumes that pre-work; honoring it.
- **V2.0** — Distribution + polish — ⬜ deferred (gate: V1.10 done; CI infra)
  - **Why deferred:** V2.0 promises `cargo install aegis-cli`,
    `brew install aegis`, `npm install -g @aegis/cli`, GitHub
    Releases auto-publish across 5 platforms × 2 architectures.
    Every one of those needs a real artifact built from V1.10's
    Python-free state, plus CI infrastructure setup (cibuildwheel-
    equivalent for Rust, signed Homebrew tap, npm publish creds).
    None of that work makes sense before there's something to
    publish.
  - **What's actionable today:** the structure is in place. V2.0
    becomes a 2–3-session rollout once V1.10 ships — `cargo dist`
    handles the cross-compile matrix; the Homebrew tap + npm
    wrapper are <100 lines each.

### Honest summary as of 2026-04-26

| Phase | State | What's true today |
| :--- | :--- | :--- |
| V1.0 | ✅ | trace + decision data types, all Python tests still pass |
| V1.1 | ✅ | OpenAI-compatible Rust provider; Python providers untouched (test rewrite gated on V1.3) |
| V1.2 | ✅ Partial+ | `Snapshot` IO + IR model (`PatchPlan`/`Patch`/`Edit`) + `apply_edit` engine all in Rust; `Executor` + `PlanValidator` Python *classes* are the remaining V1.2 piece |
| V1.3 | ✅ Partial | sequence detectors + `IterationEvent` shim + IR model ported; full Pipeline.run() coordination port still deferred (the small piece — Planner prompt construction stays Python) |
| V1.4–V1.7 | ✅ | 10 languages, registry-driven dispatch, CLI auto-walks all extensions |
| V1.8 | ⬜ | gated on V1.3-full + API quotas |
| V1.9 | ⬜ | gated on V1.3-full |
| V1.10 | ⬜ | gated on V1.9 + 2-week soak |
| V2.0 | ⬜ | gated on V1.10 + CI infra |

The "partial" V1.2/V1.3 entries each now ship the load-bearing data
contract (`PatchPlan` / `Patch` / `Edit` are Rust ground truth) plus
the load-bearing logic (`apply_edit`'s line-aware fallback joiner is
Rust ground truth). What remains for V1.2 full is `Executor` +
`PlanValidator` Python *classes* — both a straightforward port now
that all their data dependencies live in Rust. What remains for V1.3
full is the Pipeline.run() coordination shell (~150 LOC, easily
trait-callable from Python; the ~600 LOC of Planner prompt
construction stays Python by design).

**What this means for next-session-agent:** with the IR-model port
shipped, the next clean unit of work is **`Executor` + `PlanValidator`
in Rust**. Both classes now have all their data dependencies in
`aegis-ir` + `aegis-runtime::Snapshot`; the port is structural, not
algorithmic. After that, the V1.3 coordination shell (~150 LOC,
trait-callable from Python so Planner prompts can stay Python) closes
out V1.3 full. Then V1.8 / V1.9 / V1.10 / V2.0 unblock in sequence as
their gates are met (API quotas, 2-week soak, CI infra — all
real-world, not code).

---

## How a new agent picks this up

If you are an AI coding agent reading this for the first time, do
this in order:

1. **Read [`AGENTS.md`](../AGENTS.md)** at the repo root for project
   framing constraints.

2. **Read this entire plan**. Trade-offs in
   [Risks and mitigations](#risks-and-mitigations) were resolved
   deliberately; flag re-decisions as plan updates, not
   implementation choices.

3. **Read the framing references** that survive the port unchanged:
   - [`README.md` Design principles](../README.md#design-principles)
   - [`docs/post_launch_discipline.md`](post_launch_discipline.md)
   - [`docs/gap3_control_plane.md`](gap3_control_plane.md) Critical
     Principle (the no-retry-engine rule applies to the Rust port too)

4. **Identify which phase to do next**:
   - Check the [Phase status](#phase-status) section of this file
   - The next phase is the lowest-numbered one without ✅ Done
   - Make sure the previous phase's exit gate is still met (`cargo
     test --workspace`, then full Python test suite if not yet at V1.10)

5. **One commit per phase** with message: `feat(rust-port): V1.N —
   <phase title>. Per docs/v1_rust_port_plan.md.`

6. **At the end of each phase**, update this document:
   - Tick the phase box with commit hash + date
   - Update README's "Status" table if user-visible
     change
   - Open follow-up issues for things you discovered the plan
     didn't anticipate

---

## Plan-document maintenance

This document is the single source of truth for the Rust port. If
reality diverges from this plan (a phase turns out harder, a
dependency breaks, a phase ordering needs to change), **update this
document in the same commit that addresses the divergence**.

PRs that change implementation but not this document on Rust-port
matters will be asked to update the plan first.

---

## Relationship to `multi_language_plan.md`

This plan **supersedes** `docs/multi_language_plan.md`. Multi-
language work folds into V1.4–V1.7. The older document is preserved
for context but no longer drives scheduling — see its banner.
