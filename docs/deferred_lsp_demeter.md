# Deferred: real cross-module Demeter detection (LSP-level)

**Status:** explicitly deferred. Not on any near-term roadmap.
**Tracked as:** S7.8 in the Phase 7 task list.

## What "real" cross-module Demeter would mean

Today `cross_module_chain_count` (Phase 7 / S7.5) flags chains of
depth ≥ 3 whose leftmost identifier is either:

  1. Found in the file's `import_names` set, **or**
  2. PascalCase (heuristic for "module / class identifier")

This catches the textbook case `Order.customer.address.country` and
skips intra-class `self.x.y.z`. But it does **not** verify that
each step of the chain actually crosses a module boundary. It can
miss chains where the root is a locally-bound alias of an imported
symbol used through `attr` access where the receiver type comes
from another file. It can also false-positive on locally-defined
PascalCase classes used inside their own module.

Real cross-module detection would answer, for each `.x` in a chain:

  > Does the receiver of `.x` resolve to a type defined in a
  > **different** module than the file we're analysing?

That's a type-resolution question. Three implementation paths:

### Option A — host-language LSP integration

Use `pyright`, `tsserver`, `rust-analyzer`, etc. as a black-box type
oracle. Aegis would shell out per file (or stay connected over LSP
JSON-RPC). For each chain expression, query the LSP for the type of
the receiver and walk down.

**Cost:** many days for the orchestration alone (LSP lifecycle,
multi-language dispatch, server warm-up latency). Adds a hard
dependency on each language's LSP being installed and configured.
Breaks aegis's "single static binary, no language tooling required"
posture.

### Option B — embed a lightweight type checker per language

Vendor a partial type resolver per supported language. For Python,
use a minimal scope/symbol implementation; for TypeScript, parse
type annotations and resolve through `import` records.

**Cost:** weeks per language. Type resolution is one of the hardest
problems in static analysis once you allow generics, structural
typing, and dynamic dispatch. The result would be at best a
half-precision approximation that disagrees with the host LSP in
edge cases — and confusion about "why does the IDE say one thing
and aegis another" would erode trust.

### Option C — content with the heuristic, document it explicitly

Treat `cross_module_chain_count` as the warn-level proxy it is.
Cope with both false positives and false negatives by keeping the
signal at warn (never block alone). Encourage reviewers to confirm
flagged chains using their IDE — aegis points the finger; the IDE
proves the case.

This is the chosen path.

## Why this is a deliberate design choice, not a TODO

Aegis's discipline is "Only reject what is verifiably bad." Real
type resolution is the only way to make `cross_module_chain_count`
truly verifiable, but the cost of that verification (Option A or B)
exceeds the value gained over the current heuristic. Worse: if we
implement it incorrectly, the verdict starts disagreeing with the
canonical LSP, which means aegis is sometimes wrong on something it
claims to be sure about — that violates the discipline more
fundamentally than under-precision did.

The honest place to land is:

  - The heuristic stays at **warn** severity (S7.5 already does this).
  - Reviewers know it's a heuristic (this doc).
  - When type-aware static analysis becomes tractable to embed
    cheaply (e.g., a future sub-MB Rust crate that handles Python
    + TS), revisit. Not before.

## What we DID do instead (S7.5–S7.7)

- `cross_module_chain_count` — chain depth + import-set + PascalCase
  heuristic, warn-level only.
- `import_usage_count` (S7.6) — directly attributes member-accesses
  to imported names. Without resolving types, this gives a real
  picture of which imports the file leans on heavily.
- `type_leakage_count` (S7.6, hardened) — only counts types that
  actually match an imported name, dropping the heuristic count of
  any annotated parameter.
- `signal_z_score` (S7.7) — generic project-wide stats on every
  numeric signal so a single high `cross_module_chain_count` can
  be compared against the workspace baseline.

These together get most of the value Option A/B would provide,
without buying their costs. We will not implement S7.8 until the
cost-benefit changes.
