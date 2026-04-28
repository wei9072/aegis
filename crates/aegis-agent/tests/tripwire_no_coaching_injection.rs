//! AEGIS V3 NEGATIVE-SPACE TRIPWIRE — no coaching-injection APIs
//! ==============================================================
//!
//! ## What this is (and is NOT)
//!
//! This file is a **tripwire**, not algorithmic enforcement. Its
//! purpose is to **surface the framing question to PR review** when
//! someone introduces an API that converts verifier verdicts,
//! stalemate signals, or aegis-check verdicts into hint strings to
//! be fed back into the next-turn prompt.
//!
//! Verdicts are observation, displayed to the user. The user (not
//! the agent) decides whether to refine the task description and
//! run another turn.
//!
//! ## What it catches
//!
//! - `fn` definitions whose identifiers match known coaching-API
//!   patterns (`fn verdict_to_hint`, `fn coach_from`, ...).
//! - `struct` / `enum` definitions whose identifiers match known
//!   coaching-shaped types (`AutoHinter`, `VerdictCoach`, ...).
//!
//! ## What it does NOT catch
//!
//! Renaming bypasses succeed silently. The tripwire is signaling,
//! not fortress. See `tripwire_no_auto_retry.rs` for the same
//! framing rationale at length — negative-space contracts ("no
//! function with shape X exists") are not expressible in Rust's
//! type system, so AST-based identifier scanning is the cheapest
//! enforcement that makes drift visible to PR reviewers without
//! pretending to prevent it.
//!
//! ## How it works
//!
//! Every `.rs` file under `src/` is parsed with `syn`; function and
//! type identifiers are collected from the AST. Comments,
//! docstrings, and string literals are ignored — identifiers are
//! the only signal.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// Function identifiers forbidden anywhere in `src/`. A function
/// with one of these names would imply Aegis is converting a
/// verdict into a coaching prompt for the next turn.
const FORBIDDEN_FN_IDENTS: &[&str] = &[
    "verdict_to_hint",
    "coach_from",
    "inject_feedback",
    "build_hint_from",
    "auto_correction_prompt",
    "rewrite_task_with_verdict",
    "prompt_from_verdict",
    "next_prompt_from_failure",
];

/// Type identifiers (struct / enum) forbidden anywhere in `src/`.
/// A type with one of these names would imply we've designed a
/// coaching channel.
const FORBIDDEN_TYPE_IDENTS: &[&str] = &[
    "AutoHinter",
    "VerdictCoach",
    "FeedbackInjector",
    "RetryPromptBuilder",
];

#[test]
fn no_function_definitions_match_forbidden_coaching_names() {
    let names = collect_fn_names();
    for forbidden in FORBIDDEN_FN_IDENTS {
        assert!(
            !names.contains(*forbidden),
            "src/ defines fn {forbidden} — see this file's module docs \
             for the framing rationale"
        );
    }
}

#[test]
fn no_type_definitions_match_forbidden_coaching_names() {
    let names = collect_type_names();
    for forbidden in FORBIDDEN_TYPE_IDENTS {
        assert!(
            !names.contains(*forbidden),
            "src/ defines type {forbidden} — see this file's module docs \
             for the framing rationale"
        );
    }
}

// ----------------------------------------------------------------
// AST helpers (shared shape with `tripwire_no_auto_retry.rs`)
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

fn collect_type_names() -> HashSet<String> {
    let mut names = HashSet::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in walk_rs(&src) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = syn::parse_file(&text) else {
            continue;
        };
        let mut collector = TypeCollector { names: &mut names };
        collector.visit_file(&file);
    }
    names
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

struct TypeCollector<'a> {
    names: &'a mut HashSet<String>,
}

impl<'ast> Visit<'ast> for TypeCollector<'_> {
    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.names.insert(i.ident.to_string());
    }
    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.names.insert(i.ident.to_string());
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
