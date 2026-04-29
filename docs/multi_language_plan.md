# Multi-language support plan

> **⚠️ SUPERSEDED — V1.4–V1.7 of the Rust port shipped this work
> in 2026-04-26.** All 10 Tier A languages (Python, TypeScript,
> JavaScript, Go, Java, C#, PHP, Swift, Kotlin, Dart) are live in
> the Rust workspace; run `aegis languages` for the live registry.
> **2026-04-27** added Rust as Tier A #11 so aegis can dogfood itself.
> Per-phase status is at the bottom of this file.
>
> **What survives as canonical:** the `LanguageAdapter` trait shape
> (now in `crates/aegis-core/src/ast/adapter.rs`), the per-language
> checklist below, and the risks list. The Python module / `pytest`
> / `pip install -e .` references throughout are V0.x history —
> the Rust equivalents are `cargo test --workspace` and adapter
> files under `crates/aegis-core/src/ast/languages/`.
>
> **Don't drive new work from this file.** For Tier B (Vue / Angular
> / Flutter) follow-ups, see [`ROADMAP.md`](ROADMAP.md) backlog.

---

> **Status:** Plan only — no code in this commit. Authored 2026-04-26.
> A new agent picking this up should read this file end-to-end before
> touching `aegis-core-rs/`. The plan is intentionally complete enough
> that re-derivation is unnecessary; if you find yourself re-deriving
> a decision the plan made, the plan is incomplete and should be
> updated.

---

## Goal

Aegis V0.x supports **Python only** at "tier 2" (Ring 0 + structural
signals + PolicyEngine). Non-Python projects (React, Node, Go, Java,
etc.) currently get a partial experience at best.

This plan extends tier 2 support to **14 popular languages /
frameworks** so that integration paths A (pre-commit), B (CI), and
C (MCP) work meaningfully across the broad LLM-coding audience.

The plan is **scope-only for tier 2**. Multi-turn pipeline (PatchPlan,
scenarios, verifiers) stays Python-bound — see
[Out of scope](#out-of-scope) for why and when that decision could
flip.

---

## Scope — 14 targets, two tiers of work

### Tier A — Real languages (need own tree-sitter grammar + adapter)

| # | Language | Tree-sitter crate | Notes |
| :-- | :-- | :-- | :-- |
| 1 | **Python** | `tree-sitter-python` (have) | already supported — used as reference implementation |
| 2 | **TypeScript** | `tree-sitter-typescript` (have) | covers `.ts` + `.tsx` (the crate exposes both) |
| 3 | **JavaScript** | `tree-sitter-javascript` (need to add) | covers `.js` + `.jsx`; large delta-from-TS would be wasteful, share most code |
| 4 | **Go** | `tree-sitter-go` (need to add) | empty `queries/go.scm` placeholder exists |
| 5 | **Java** | `tree-sitter-java` (need to add) | enterprise stack baseline |
| 6 | **C#** | `tree-sitter-c-sharp` (need to add) | namespace + using directive imports |
| 7 | **PHP** | `tree-sitter-php` (need to add) | mixed-content handling deferred (see scope notes) |
| 8 | **Swift** | `tree-sitter-swift` (need to add) | iOS / macOS code |
| 9 | **Kotlin** | `tree-sitter-kotlin` (community-maintained, verify quality) | Android + JVM |
| 10 | **Dart** | `tree-sitter-dart` (community-maintained) | needed for Flutter (Tier B #14) |
| 11 | **Rust** | `tree-sitter-rust` (added 2026-04-27) | aegis self-dogfood — without it `aegis scan` skips its own crates |

### Tier B — Frameworks (specialisation on a Tier A language)

| # | Framework | Underlying language | What's specialisation | Phase |
| :-- | :-- | :-- | :-- | :-- |
| 11 | **React** | TypeScript / JavaScript | JSX / TSX syntax — `tree-sitter-typescript`'s `tsx` parser handles this | folded into Tier A #2/#3 |
| 12 | **Vue.js** | JavaScript / TypeScript inside `.vue` SFC | `<script>` block parsing inside Single File Components | own phase (P6) |
| 13 | **Angular** | TypeScript with decorators + HTML templates | TS works via #2; templates need `tree-sitter-html` | own phase (P6) |
| 14 | **Node.js** | JavaScript / TypeScript | runtime, not a separate language | folded into Tier A #2/#3 |
| (15) | **Flutter** | Dart | Dart works via Tier A #10; widget tree analysis is framework specialisation | own phase (P6, optional) |

**Tier B reality check:** Vue SFC and Angular templates are
*genuinely different* parsing problems (mixed-content files). They
require either multi-language parsing inside one file OR pre-processing
to extract the script block. Both are deferred to P6; P0–P5 deliver
all the Tier A languages first.

---

## Architecture

The current code embeds `tree-sitter-python` directly in
`aegis-core-rs/src/ast/parser.rs`. That doesn't scale to 14 languages
— each new language would mean editing `analyze_file()`. The
refactor in **Phase 0** introduces a clean abstraction.

### Abstraction 1 — `LanguageAdapter` trait (new)

New file: `aegis-core-rs/src/ast/adapter.rs`.

```rust
use tree_sitter::Language;

pub trait LanguageAdapter: Send + Sync {
    /// Stable name — "python", "typescript", "go", ...
    /// Used in error messages and Python-side dispatch.
    fn name(&self) -> &'static str;

    /// File extensions this adapter handles — [".py"], [".ts", ".tsx"], etc.
    /// Lowercase, with leading dot.
    fn extensions(&self) -> &'static [&'static str];

    /// The tree-sitter grammar.
    fn tree_sitter_language(&self) -> Language;

    /// Tree-sitter query: capture all imported / required modules.
    /// Capture name MUST be `@import`. The captured node's text is
    /// taken as the module identifier.
    fn import_query(&self) -> &'static str;

    /// Walk the AST root and return the longest method-chain depth.
    /// Default impl works on `member_expression`-shaped nodes; per-
    /// language overrides handle non-OO syntax (Go composite calls,
    /// Swift trailing closures, etc.).
    fn max_chain_depth(&self, root: tree_sitter::Node, source: &[u8]) -> usize {
        crate::ast::chain_depth::default_chain_depth(root, source)
    }
}
```

Each language gets a struct implementing this trait:

```rust
// aegis-core-rs/src/ast/languages/python.rs
pub struct PythonAdapter;
impl LanguageAdapter for PythonAdapter {
    fn name(&self) -> &'static str { "python" }
    fn extensions(&self) -> &'static [&'static str] { &[".py"] }
    fn tree_sitter_language(&self) -> Language { tree_sitter_python::language() }
    fn import_query(&self) -> &'static str {
        include_str!("../../../queries/python.scm")
    }
    // max_chain_depth: use default impl (Python's `obj.attr.attr` is
    // member_expression-shaped, default works)
}
```

Adding a language = adding a struct. No modifications to `parser.rs`
or signal extractors.

### Abstraction 2 — Registry (new)

New file: `aegis-core-rs/src/ast/registry.rs`.

```rust
use std::sync::OnceLock;

static REGISTRY: OnceLock<LanguageRegistry> = OnceLock::new();

pub struct LanguageRegistry {
    adapters: Vec<Box<dyn LanguageAdapter>>,
}

impl LanguageRegistry {
    pub fn global() -> &'static LanguageRegistry {
        REGISTRY.get_or_init(|| LanguageRegistry::default_set())
    }

    /// Build the canonical registry. New languages added here.
    fn default_set() -> Self {
        Self {
            adapters: vec![
                Box::new(crate::ast::languages::python::PythonAdapter),
                Box::new(crate::ast::languages::typescript::TypeScriptAdapter),
                // ... new languages added here
            ],
        }
    }

    pub fn for_path(&self, path: &str) -> Option<&dyn LanguageAdapter> {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))?;
        self.adapters.iter()
            .find(|a| a.extensions().iter().any(|x| *x == ext))
            .map(|b| b.as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.name()).collect()
    }
}
```

### Abstraction 3 — IR-based signal computation

The existing `signals/coupling.rs`, `signals/demeter.rs` should
operate on `IrNode` (already partially the case via `build_ir`).
Per-language work is to map AST → IR; signals stay language-agnostic.

`fan_out` = `len(unique imports)` from `import_query` — works for
all languages once each adapter implements the query.

`max_chain_depth` = trait method with sensible default + per-language
override when needed.

`circular_dependency` (Ring 0) = graph cycle on the import graph,
language-agnostic once `extract_imports` returns full module names.

### Refactor: `analyze_file` becomes adapter dispatch

```rust
// aegis-core-rs/src/ast/parser.rs (post-refactor)
#[pyfunction]
pub fn analyze_file(filepath: &str) -> PyResult<AstMetrics> {
    let code = std::fs::read_to_string(filepath)?;
    let registry = crate::ast::registry::LanguageRegistry::global();
    let adapter = registry.for_path(filepath).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "no language adapter for path {filepath:?} (supported: {:?})",
            registry.names(),
        ))
    })?;
    let mut parser = Parser::new();
    parser.set_language(adapter.tree_sitter_language()).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    let root = tree.root_node();
    Ok(AstMetrics {
        has_syntax_error: root.has_error(),
        fan_out: count_imports(adapter, root, code.as_bytes()),
        max_chain_depth: adapter.max_chain_depth(root, code.as_bytes()),
    })
}
```

### What stays Python-only (and why)

| Layer | Status | Reason |
| :--- | :--- | :--- |
| `aegis/runtime/pipeline.py` (multi-turn loop) | Python-only | `PatchPlan` anchor matching assumes Python AST |
| `aegis/runtime/executor.py` | Generic file IO | Should already work for any text file — verify in P0 |
| `aegis/runtime/validator.py` (PlanValidator) | Python-only | Calls `ast.parse` on patched file; would need pluggable validator per language |
| `aegis/runtime/task_verifier.py` (Layer C) | Generic | Per-scenario verifiers; new scenarios in non-Python languages would write their own |
| `aegis/agents/planner.py` | Python prompt-bound | Planner asks LLM for Python patches; multi-language prompting is V2+ |
| `aegis/eval/`, `tests/scenarios/` | Python-only | All 4 scenarios target Python files |

In short: **structural analysis (Ring 0, signals, policy) becomes
multi-language. Multi-turn refactoring stays Python-only.**

This matches what non-Python users actually want from Aegis — a lint-
style enforcement layer at commit / PR / agent-decision time, not a
refactor agent that mutates their TypeScript.

---

## Phased delivery plan

Each phase has an entry gate (what must already be true) and an exit
gate (what must be true to consider it done). The exit gate of phase
N is the entry gate of phase N+1.

### Phase 0 — Refactor existing Python to use the trait (no behavior change)

**Entry gate:** Python tier 2 works (current state).

**Work:**
1. Create `aegis-core-rs/src/ast/adapter.rs` with `LanguageAdapter` trait
2. Create `aegis-core-rs/src/ast/registry.rs` with `LanguageRegistry`
3. Refactor `python.rs` to expose `PythonAdapter: LanguageAdapter`
4. Refactor `analyze_file` in `parser.rs` to use the registry
5. Refactor `enforcement.rs` Ring 0 entry points to dispatch via registry
6. Move language-agnostic `max_chain_depth` walking to a shared module

**Exit gate:**
- All 256 existing tests pass
- `python -c "from aegis import _core; print(_core.analyze_file('aegis/cli.py'))"` returns the same metrics as before
- No file in `aegis-core-rs/src/` references `tree_sitter_python` outside `languages/python.rs`

**Estimated session count:** 1 (3-5 hours focused).

### Phase 1 — TypeScript + JavaScript

**Entry gate:** Phase 0 exit gate met.

**Work:**
1. Implement `TypeScriptAdapter` in `aegis-core-rs/src/ast/languages/typescript.rs` (overwrite the current 15-line file)
2. Implement `JavaScriptAdapter` in `aegis-core-rs/src/ast/languages/javascript.rs` (new — share import query patterns where applicable)
3. Add `tree-sitter-javascript` to `aegis-core-rs/Cargo.toml`
4. Write `aegis-core-rs/queries/typescript.scm` (overwrite empty placeholder)
5. Write `aegis-core-rs/queries/javascript.scm`
6. Register both adapters in `LanguageRegistry::default_set()`
7. Tests:
   - `tests/test_ts_signals.py` — fan_out, max_chain_depth on a sample TS/TSX file
   - `tests/test_ts_ring0.py` — syntax_valid, syntax_invalid, circular_dependency
   - `tests/test_js_signals.py`, `tests/test_js_ring0.py` — same shape for JS/JSX
8. Sanity test: clone a small open-source React component, run
   `aegis check src/Component.tsx`, confirm it produces a verdict

**Exit gate:**
- All Python tests still pass
- New TS + JS tests pass
- `aegis check foo.ts` and `aegis check foo.js` produce sensible Ring 0 output
- README + AGENTS.md updated to list TS + JS as supported

**Estimated session count:** 1 (TS + JS share most logic).

### Phase 2 — Go

**Entry gate:** Phase 1 exit gate met (proves the pattern).

**Work:**
1. `tree-sitter-go` to Cargo.toml
2. `GoAdapter` in `aegis-core-rs/src/ast/languages/go.rs`
3. `aegis-core-rs/queries/go.scm` (replace empty placeholder)
4. Register in registry
5. Tests: `tests/test_go_signals.py` + `tests/test_go_ring0.py`
6. Sanity: run on a small Go file with imports + structs

**Note on Go-specific:** Go imports use `import "package/path"`
syntax, no aliases by default. `max_chain_depth` for Go: typical
chain is `obj.field.field`, default trait impl should work.

**Exit gate:** standard.

**Estimated session count:** 1.

### Phase 3 — Java + C#

**Entry gate:** Phase 2 exit gate met.

**Work:** parallel implementation of Java and C# (both OO, both have
`import` / `using` directives, both work with default chain depth).

1. Tree-sitter crates: `tree-sitter-java`, `tree-sitter-c-sharp`
2. Adapters in `languages/java.rs`, `languages/csharp.rs`
3. Queries: `java.scm`, `csharp.scm`
4. Tests for each
5. Sanity: small Java file + small C# file

**Java-specific:** `import com.x.Y;` and `import com.x.*;` both
count as single dep. Annotations might be parsed as method calls —
verify `max_chain_depth` doesn't over-count.

**C#-specific:** `using System;` is the import equivalent.
`var x = a?.b?.c` (null-conditional) is still chain depth 3.

**Exit gate:** standard.

**Estimated session count:** 1-2 (parallel work).

### Phase 4 — PHP + Swift + Kotlin + Dart

**Entry gate:** Phase 3 exit gate met.

**Work:** four parallel adapters, same pattern. Each one ~half a
session.

**PHP-specific gotcha:** PHP files mix HTML and PHP. For V0.5, check
only files with `<?php` opening tag and treat as PHP throughout. Mixed-
content PHP/HTML is deferred to P6.

**Swift-specific:** trailing closures (`f { ... }`) might confuse
chain-depth counting. Verify with sample.

**Kotlin-specific:** `tree-sitter-kotlin` is community-maintained.
Verify it parses real Kotlin files cleanly before committing.

**Dart-specific:** `tree-sitter-dart` is also community-maintained.
Same verification needed.

**Exit gate:** standard.

**Estimated session count:** 2.

### Phase 4.5 — Rust (2026-04-27)

**Why retroactively:** the original plan stopped at Dart because the
audience was assumed to be polyglot end-users. On 2026-04-27 aegis
became a Rust workspace that wanted to scan itself, and "skip 1,300
of your own files" was not acceptable. Adding Rust is dogfood, not
feature.

**Files added:**
- `Cargo.toml`: `tree-sitter-rust = "0.20"` (one line)
- `crates/aegis-core/queries/rust.scm`: 3 captures (`use_declaration`,
  `mod_item`, `extern_crate_declaration`)
- `crates/aegis-core/src/ast/languages/rust.rs`: ~50 LOC adapter
  with custom `normalize_import` that takes the leftmost `::`
  segment (so `use std::io::Read` resolves to `std`)
- `crates/aegis-core/src/ast/adapter.rs`: added `field_expression`
  + `try_expression` to the chain-depth walker (Rust-specific node
  kinds — additive change, no other language affected)
- `crates/aegis-core/src/ast/languages/mod.rs` + `registry.rs`:
  one-line registrations

**Verified:** `aegis scan --workspace .` on aegis itself — 111
`.rs` files, 73 import edges, 0 cycles, 0 syntax errors. Cache
warm path 2 ms.

**Rust-specific gotcha:** crate-internal cycle detection is
best-effort. Rust uses `crate::`, `super::`, `self::` for
intra-crate references which the stem-match cycle detector can't
resolve to file paths. `mod foo;` is captured (catches the
file-include shape), and external-crate cycles still light up.
Deep intra-crate cycle resolution would require Cargo workspace
metadata — out of scope.

### Phase 5 — Acceptance review

Not implementation — pause to assess:

- Are the new languages actually being used? (check GitHub issues, CI usage)
- Are signals (`fan_out`, `max_chain_depth`) producing meaningful values across all 10 languages?
- Are PolicyEngine thresholds (e.g. `fan_out > 20`) appropriate for non-Python languages?

If thresholds need per-language tuning, that's tier-3 work — emit
`PolicyConfig` per language, default to Python's table.

### Phase 6 — Framework specialisation (optional, evidence-driven)

**Entry gate:** Phase 4 done + at least 2 issues asking for framework support.

**Work, in order of demand:**
1. **React JSX/TSX** — already covered by Phase 1 (tree-sitter-typescript handles tsx)
2. **Vue.js SFC** — extract `<script>` block, parse with TS or JS adapter. Requires either custom pre-parser or `tree-sitter-vue`.
3. **Angular** — TS via Phase 1 covers code; templates use `tree-sitter-html`; component-level signals (decorators → injection complexity) are framework-specific tier 3
4. **Flutter widget analysis** — Dart works via Phase 4; widget tree depth is framework-specific

Each framework specialisation should be its own commit with explicit
"why this is justified" framing in the commit message.

---

## Per-language work checklist (template)

When adding a new language, complete every box. The template lives
in this document so a new agent can read it once and follow it for
each language.

```
[ ] 1. Add tree-sitter crate to aegis-core-rs/Cargo.toml
[ ] 2. Create aegis-core-rs/src/ast/languages/<lang>.rs with <Lang>Adapter
[ ] 3. Create aegis-core-rs/queries/<lang>.scm with @import captures
[ ] 4. Verify max_chain_depth default works; if not, override in adapter
[ ] 5. Register adapter in LanguageRegistry::default_set()
[ ] 6. Add tests/test_<lang>_signals.py:
       [ ] fan_out from N distinct imports
       [ ] max_chain_depth from a known method-chain
       [ ] no false positives on simple file
[ ] 7. Add tests/test_<lang>_ring0.py:
       [ ] valid file → syntax_valid
       [ ] invalid file → syntax_invalid with line number
       [ ] circular import detection
[ ] 8. Sanity test on real file:
       [ ] download or write 1 representative file from real-world code
       [ ] run `aegis check <file>` and `python -c "from aegis import _core; print(_core.analyze_file('<file>'))"`
       [ ] verify output is sensible (not zero, not unbounded)
[ ] 9. Update README.md "Status" section to list <lang> as supported
[ ] 10. Update AGENTS.md "Where things are" to mention <lang> support
[ ] 11. Run full test suite: `pip install -e . && pytest tests/ -q` — all passing
[ ] 12. Commit with message: feat(lang): add <Lang> tier 2 support
```

---

## Testing strategy

### Unit-test coverage per language (minimum)

Each language's `tests/test_<lang>_signals.py` and `tests/test_<lang>_ring0.py`
must include:

| Test case | What it pins |
| :--- | :--- |
| `test_fan_out_counts_unique_imports` | One file with N distinct imports → `fan_out == N` |
| `test_fan_out_dedupes_repeated_imports` | Same import twice → `fan_out == 1` |
| `test_max_chain_depth_simple` | `a.b.c` → `max_chain_depth == 3` |
| `test_max_chain_depth_multiple_chains` | Two chains of different depths → max wins |
| `test_syntax_valid` | Well-formed file → no Ring 0 violation |
| `test_syntax_invalid` | Malformed file → Ring 0 violation with line number |
| `test_circular_import` | A imports B imports A → Ring 0 cycle violation |

### Cross-language regression

A meta-test at `tests/test_language_registry.py`:
- For every registered adapter, calling `analyze_file` on an empty
  file (with the right extension) returns `AstMetrics` without
  panicking
- For every registered adapter, fan_out on a 0-import file is 0
- For every registered adapter, the import_query compiles cleanly
  against the tree-sitter language

This test grows as adapters are added — single failure point catches
"forgot to register" bugs.

### Sanity test files

Maintain a `tests/fixtures/<lang>/` directory with:
- `simple.<ext>` — minimal valid file
- `with_imports.<ext>` — known fan_out for assertions
- `chained.<ext>` — known max_chain_depth
- `broken.<ext>` — intentional syntax error

These also serve as the agent's reference for "what does well-formed
look like" when implementing each adapter.

---

## Documentation updates per phase

### After each phase

- README.md "Status" table: add languages added in this phase
- AGENTS.md "Where things are" cheatsheet: mention new language adapter location
- This file (`docs/multi_language_plan.md`): tick the checkbox for completed phase, append "✅ Done" with commit hash

### After Phase 1 (first new language)

- README.md "Build note": no change (install sequence stays the same — adding languages doesn't add build steps)
- AGENTS.md "Setup" section: no change
- New section in README.md: **"Supported languages"** — table mapping language → status (✅ tier 2 / 🟡 partial / ❌ not yet)

### After Phase 5 (acceptance review)

- Update `docs/v1_validation.md` if multi-language has produced new evidence (e.g. signals on TS files reveal a different cost distribution)
- Update `docs/post_launch_discipline.md` deferral list — multi-language is no longer deferred

---

## Risks and constraints

### Tree-sitter grammar quality varies

**Risk:** community-maintained grammars (Kotlin, Dart, Vue) may have
parse failures on real-world files.

**Mitigation:** sanity-test with real files (step 8 of per-language
checklist) before considering a language "supported". If a grammar
is too unreliable, mark the language 🟡 partial in the Status table
and document specifically what doesn't work.

### `max_chain_depth` false positives in non-OO languages

**Risk:** Go, Rust, functional languages have method-call AST nodes
that don't represent the same semantic concept as Python's `a.b.c`.

**Mitigation:** the trait's `max_chain_depth` method is overridable.
For each non-OO language, validate the default impl against a
sanity-test file; if it over-counts, write an override.

### Increased build time

**Risk:** every tree-sitter crate adds ~1-3 seconds to `cargo build`
and binary size. With 10+ languages, fresh `pip install` could
become 1-2 minutes.

**Mitigation:** Cargo features. Each language is a feature; users
can opt out:

```toml
[features]
default = ["python", "typescript"]
all = ["python", "typescript", "javascript", "go", "java", "csharp", "php", "swift", "kotlin", "dart"]
```

`pip install -e .` defaults to the most common languages; users can
opt into more via env var or extra dep at install time. Detail in P5
acceptance review.

### Test suite explosion

**Risk:** 10 languages × 7 tests each = 70+ tests just for language
parity, on top of the 256 existing tests.

**Mitigation:** parametrise where possible — `pytest.mark.parametrize`
across (language, fixture_file) reduces duplication. Cross-language
registry test (above) catches "forgot to register" cleanly.

### Policy threshold mismatch

**Risk:** `fan_out > 20 → BLOCK` was tuned on Python idioms. A typical
Java file has more imports; this would over-fire on Java.

**Mitigation:** Phase 5 acceptance review explicitly tests this. If
threshold mismatch is real, introduce per-language `PolicyConfig`.

---

## Out of scope (and why, and when to revisit)

| Out of scope | Reason | When to revisit |
| :--- | :--- | :--- |
| Multi-turn pipeline cross-language | `PatchPlan` is Python-AST-bound; cross-language refactoring needs a separate plan format | When ≥3 user issues request "Aegis refactor my Java code" |
| Per-language scenarios | The 4 V1 scenarios assume Python; cross-language scenarios would need a scenario abstraction | When multi-turn cross-language is approved |
| Language-specific signals (e.g. "too many useState in React") | Tier 3 work; basic fan_out / max_chain_depth covers 80% of value | After P5 with usage evidence showing what users actually want flagged |
| LSP / IDE plugin per language | Out per `docs/post_launch_discipline.md` — wrappers built ON top of Aegis, not in Aegis | Indefinitely deferred |
| Adaptive policy thresholds (per-language tuning via observed distribution) | ROADMAP §4.3 Adaptive Policy work | After enough multi-language usage data exists |

---

## How a new agent picks this up

If you are an AI coding agent reading this for the first time, do
this in order:

1. **Read AGENTS.md** at the repo root for the project's framing
   constraints. The "Rules you must follow" section applies to this
   work too — no auto-retry, no prompt rewriting, no abstraction
   extraction beyond what this plan already specifies.

2. **Read this entire plan document** before touching code. The
   trade-offs in [Risks and constraints](#risks-and-constraints)
   were resolved deliberately; if you find yourself wanting to
   re-decide one, that's a flag to add to the plan, not an
   invitation to wing it.

3. **Identify which phase to do next**:
   - Run `git log --oneline | grep "lang"` to see committed phases
   - Check for "✅ Done" markers below each phase header in this
     file
   - The next phase is the lowest-numbered one without ✅ Done

4. **Before implementing, run** `pytest tests/ -q`. Establish the
   baseline test count and pass/fail. Your work shouldn't change
   the passing tests; new tests should add, not modify.

5. **Follow the per-language checklist verbatim** for each language
   in your phase. The 12 boxes are the contract.

6. **At the end of each phase**, update this document:
   - Change the phase header from "Phase N — XYZ" to "Phase N — XYZ ✅ Done (commit `<hash>`)"
   - Update the README.md "Supported languages" table
   - Open a follow-up issue if you discovered something the plan didn't anticipate

7. **One commit per phase**. Squash if your session produced multiple
   small commits. The commit message should reference this plan:

   > Phase N — adds <languages>. Per docs/multi_language_plan.md.
   > Tests: <count> new (<count> total). Sanity-tested on <real file>.

---

## Phase status

- **Phase 0** — Refactor to LanguageAdapter trait — ✅ Done (2026-04-26, V1.4)
- **Phase 1** — TypeScript + JavaScript — ✅ Done (2026-04-26, V1.5)
- **Phase 2** — Go — ✅ Done (2026-04-26, V1.6)
- **Phase 3** — Java + C# — ✅ Done (2026-04-26, V1.6)
- **Phase 4** — PHP + Swift + Kotlin + Dart — ✅ Done (2026-04-26, V1.7)
- **Phase 5** — Acceptance review — ⬜ not started (gate: real usage data)
- **Phase 6** — Vue / Angular / Flutter — ⬜ not started (evidence-gated)

**Divergences from this plan during V1.4–V1.7 implementation:**

- All 4 implementation phases shipped in a single commit because
  the per-language work is mechanically identical — adding 9
  adapters in one batch costs less than 9 separate commits and the
  registry meta-test (`every_adapter_parses_empty_file_without_panicking`,
  `every_adapter_compiles_its_import_query`) catches "forgot to
  register" / "broken grammar" cases atomically.
- `tree-sitter-kotlin` pinned to `=0.3.4` and `tree-sitter-dart`
  pinned to `=0.0.3` — newer versions of both crates target
  tree-sitter 0.21+/0.22+, which is ABI-incompatible with the
  0.20 line that the rest of the grammars use. Pinning is the
  cheapest fix; bumping the entire grammar set to 0.22 is a
  separate work item.
- `max_chain_depth` default walker chains across the union of
  member-access / call shapes for all 10 grammars rather than
  per-language overrides. Java + Dart under-count by 1 in the
  basic `a.b.c` shape — flagged 🟡 in README; per-language
  overrides land when a real-traffic signal needs them (Phase 5
  acceptance review).
- Unit-test coverage per language is light — the registry meta-
  test + a Python-side smoke test cover the "is this language
  actually wired" question; per-language `tests/test_<lang>_*.py`
  files are deferred until real users surface concrete cases that
  break.

When a phase completes, change `⬜ not started` to `✅ Done (commit <hash>, YYYY-MM-DD)`.

---

## Plan-document maintenance

This document is the single source of truth for multi-language work.
If reality diverges from this plan (a language turns out harder than
estimated, a tree-sitter crate is broken, a phase ordering needs to
change), **update this document in the same commit that addresses
the divergence**. Future agents should never have to reverse-engineer
intent from commit history.

PRs that change implementation but not this document on
multi-language matters will be asked to update the plan first.
