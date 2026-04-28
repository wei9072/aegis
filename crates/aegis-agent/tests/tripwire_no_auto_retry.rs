//! AEGIS V3 NEGATIVE-SPACE TRIPWIRE — no auto-retry semantics
//! ===========================================================
//!
//! ## What this is (and is NOT)
//!
//! This file is a **tripwire**, not algorithmic enforcement. Its
//! purpose is to **surface the framing question to PR review** when
//! someone introduces retry-flavoured code into `aegis-agent`. It
//! does not, and cannot, prevent a determined developer from adding
//! such code under a different name.
//!
//! Concretely, the tripwire fires when:
//!   1. `AgentConfig` grows a field whose identifier contains a
//!      retry-shaped substring.
//!   2. `AgentTurnResult` grows a field a caller could consume as
//!      a retry trigger (`retry_count`, `next_action`, etc.).
//!   3. `src/` defines a `fn` whose identifier matches a known
//!      retry / coaching pattern (`fn auto_retry`, `fn coach_from`).
//!
//! ## What it does NOT catch
//!
//! All of these bypasses succeed silently:
//!   * Renaming `fn auto_retry` to `fn self_heal` / `fn try_again` /
//!     anything else not on the forbidden list.
//!   * Embedding a retry loop inside an existing function (no new
//!     identifier introduced).
//!   * Adding a retry-shaped closure or match arm.
//!
//! These bypasses are real. The tripwire's job is **signaling**:
//! a careless or drifting developer trips it, a determined one
//! routes around it. Negative-space contracts ("no function with
//! shape X exists in this crate") are not expressible in Rust's
//! type system — visibility / sealed traits constrain *who can
//! call* what exists, not *what is allowed to exist* — so AST
//! identifier scanning is the cheapest enforcement that surfaces
//! the framing question to a human reviewer.
//!
//! ## How it works (vs the V0.x string-grep version)
//!
//! Every `.rs` file under `src/` is parsed with `syn`; function and
//! field identifiers are collected from the AST, not regex-matched
//! against text. Comments, docstrings, and string literals do not
//! produce false positives.
//!
//! The struct-field check pulls field names from the actual struct
//! definition (parsed via syn). The previous version compared a
//! hand-listed array of "allowed" names to a hand-listed array of
//! "forbidden" substrings — a tautology. Adding a `retry_count: u32`
//! field to `AgentConfig` will now fire this tripwire regardless of
//! whether anyone updates an external list.
//!
//! ## See also
//!   * `docs/post_launch_discipline.md` (deferral #5)
//!   * `docs/gap3_control_plane.md` (Critical Principle)
//!   * `tests/tripwire_no_coaching_injection.rs` (sibling tripwire)
//!   * `tests/verifier_drives_done.rs` (true type-system contract —
//!     enum variant existence is compile-time enforced there)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// Substrings forbidden in `AgentConfig` / `AgentTurnResult` field
/// identifiers.
const FORBIDDEN_FIELD_SUBSTRINGS: &[&str] = &[
    "retry",
    "auto_retry",
    "max_retries",
    "retry_on_failure",
    "feedback",
    "hint",
    "advice",
    "guidance",
    "coaching",
    "next_action",
];

/// Function identifiers forbidden anywhere in `src/`.
const FORBIDDEN_FN_IDENTS: &[&str] = &[
    "auto_retry",
    "retry_on",
    "coach_from",
    "verdict_to_hint",
    "inject_feedback",
];

#[test]
fn agent_config_has_no_retry_fields() {
    let fields = collect_struct_field_names("AgentConfig");
    assert!(
        !fields.is_empty(),
        "AgentConfig struct not found in src/ — tripwire is broken"
    );
    for field in &fields {
        for forbidden in FORBIDDEN_FIELD_SUBSTRINGS {
            assert!(
                !field.to_lowercase().contains(forbidden),
                "AgentConfig field {field:?} contains forbidden substring \
                 {forbidden:?} — see this file's module docs for the \
                 framing rationale"
            );
        }
    }
}

#[test]
fn agent_turn_result_has_no_retry_fields() {
    let fields = collect_struct_field_names("AgentTurnResult");
    assert!(
        !fields.is_empty(),
        "AgentTurnResult struct not found in src/ — tripwire is broken"
    );
    for field in &fields {
        for forbidden in FORBIDDEN_FIELD_SUBSTRINGS {
            assert!(
                !field.to_lowercase().contains(forbidden),
                "AgentTurnResult field {field:?} contains forbidden substring \
                 {forbidden:?} — see this file's module docs for the \
                 framing rationale"
            );
        }
    }
}

#[test]
fn no_function_definitions_match_forbidden_retry_names() {
    let names = collect_fn_names();
    for forbidden in FORBIDDEN_FN_IDENTS {
        assert!(
            !names.contains(*forbidden),
            "src/ defines fn {forbidden} — see this file's module docs \
             for the framing rationale. If the function is genuinely \
             needed, change the framing first (and the doc strings \
             above) before adding it."
        );
    }
}

/// Sanity probe — the genuine type-system layer, not a tripwire.
/// `AgentConfig::default()` must remain constructible with no
/// retry-shaped argument. If a refactor adds a required field
/// `max_retries: u32`, this test fails to **compile**, which is the
/// real architectural enforcement (vs the substring/AST tripwires
/// above, which only signal).
#[test]
fn agent_config_constructible_without_retry_args() {
    let cfg = aegis_agent::AgentConfig::default();
    assert_eq!(cfg.max_iterations_per_turn, 0);
    assert!(cfg.session_cost_budget.is_none());
    assert!(cfg.workspace_root.is_none());
}

// ----------------------------------------------------------------
// AST helpers (shared shape with `tripwire_no_coaching_injection.rs`)
// ----------------------------------------------------------------

fn collect_fn_names() -> HashSet<String> {
    let mut names = HashSet::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in walk_rs(&src) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = syn::parse_file(&text) else {
            continue;
        };
        let mut collector = FnCollector { names: &mut names };
        collector.visit_file(&file);
    }
    names
}

fn collect_struct_field_names(target: &str) -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in walk_rs(&src) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = syn::parse_file(&text) else {
            continue;
        };
        for item in &file.items {
            if let syn::Item::Struct(s) = item {
                if s.ident == target {
                    if let syn::Fields::Named(named) = &s.fields {
                        return named
                            .named
                            .iter()
                            .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
                            .collect();
                    }
                }
            }
        }
    }
    Vec::new()
}

struct FnCollector<'a> {
    names: &'a mut HashSet<String>,
}

impl<'ast> Visit<'ast> for FnCollector<'_> {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.names.insert(i.sig.ident.to_string());
        syn::visit::visit_item_fn(self, i);
    }
    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.names.insert(i.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, i);
    }
    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.names.insert(i.sig.ident.to_string());
        syn::visit::visit_trait_item_fn(self, i);
    }
}

fn walk_rs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_rs_inner(dir, &mut out);
    out
}

fn walk_rs_inner(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_inner(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}
