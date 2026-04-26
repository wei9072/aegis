//! Pure edit-application logic shared by Validator (virtual-fs
//! simulation) and Executor (real write). No I/O. Deterministic.
//!
//! Single source of truth for the
//! `APPLIED / ALREADY_APPLIED / AMBIGUOUS / NOT_FOUND` semantics.
//! Mirrors `aegis/shared/edit_engine.py` line-for-line so the Python
//! re-export keeps every existing test green.
//!
//! Anchored semantics (preferred — used whenever `context_before` or
//! `context_after` is non-empty):
//!   - `context_before + old_string + context_after` uniquely present
//!     -> `Applied`, return modified content.
//!   - `context_before + new_string + context_after` uniquely present
//!     (and old anchor absent) -> `AlreadyApplied`.
//!   - Either anchor appears multiple times -> `Ambiguous`.
//!   - Neither anchor present -> `NotFound`.
//!
//! The matcher is **line-aware**: the Planner schema describes
//! `context_before` / `context_after` as "surrounding lines", and
//! LLMs typically emit them as bare lines (no leading/trailing
//! newline) — but the file holds newlines between those lines. A
//! pure byte-concat matcher would `NotFound` a perfectly correct
//! plan (this was the root cause of the syntax_fix scenario refusing
//! to converge: gemma-4-31b-it produced
//! `context_after = "    return a + b"` for `def add(a, b)`, but the
//! file holds `def add(a, b)\n    return a + b`).
//!
//! Resolution: try two candidate joinings — raw concat (for inline
//! anchors like `x = 1 -> x = 10`) and newline-joined (for the
//! line-level anchors LLMs naturally produce). The first candidate
//! that yields a unique match wins. Raw is tried first so all
//! pre-existing inline-anchor behaviour stays bit-identical.

use crate::patch::{Edit, PatchStatus};

/// Outcome of one `apply_edit` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditResult {
    pub status: PatchStatus,
    pub matches: usize,
}

impl EditResult {
    fn new(status: PatchStatus, matches: usize) -> Self {
        Self { status, matches }
    }
}

/// Apply a single edit to an in-memory string.
///
/// Returns `(new_content, result)`. When `result.status` is anything
/// other than `Applied`, `new_content == content`.
pub fn apply_edit(content: &str, edit: &Edit) -> (String, EditResult) {
    if edit.old_string.is_empty() {
        return (content.to_string(), EditResult::new(PatchStatus::NotFound, 0));
    }

    let has_context = !edit.context_before.is_empty() || !edit.context_after.is_empty();
    if has_context {
        let candidates = candidate_joiners(
            &edit.context_before,
            &edit.old_string,
            &edit.context_after,
        );
        let pairs: Vec<(String, String)> = candidates
            .iter()
            .map(|joiner| {
                let old_anchor = joiner(&edit.context_before, &edit.old_string, &edit.context_after);
                let new_anchor = joiner(&edit.context_before, &edit.new_string, &edit.context_after);
                (old_anchor, new_anchor)
            })
            .collect();

        // Pass 1: any candidate whose old_anchor matches the file?
        for (old_anchor, new_anchor) in &pairs {
            let old_count = count_substring(content, old_anchor);
            if old_count == 1 {
                return (
                    replace_first(content, old_anchor, new_anchor),
                    EditResult::new(PatchStatus::Applied, 1),
                );
            }
            if old_count > 1 {
                return (
                    content.to_string(),
                    EditResult::new(PatchStatus::Ambiguous, old_count),
                );
            }
        }

        // Pass 2: was the edit already applied?
        for (_, new_anchor) in &pairs {
            let new_count = count_substring(content, new_anchor);
            if new_count == 1 {
                return (
                    content.to_string(),
                    EditResult::new(PatchStatus::AlreadyApplied, 1),
                );
            }
            if new_count > 1 {
                return (
                    content.to_string(),
                    EditResult::new(PatchStatus::Ambiguous, new_count),
                );
            }
        }

        return (content.to_string(), EditResult::new(PatchStatus::NotFound, 0));
    }

    // Unanchored fallback: old_string alone must be unique.
    let raw_count = count_substring(content, &edit.old_string);
    if raw_count == 1 {
        return (
            replace_first(content, &edit.old_string, &edit.new_string),
            EditResult::new(PatchStatus::Applied, 1),
        );
    }
    if raw_count > 1 {
        return (
            content.to_string(),
            EditResult::new(PatchStatus::Ambiguous, raw_count),
        );
    }
    (content.to_string(), EditResult::new(PatchStatus::NotFound, 0))
}

/// Sequentially apply edits. Each edit sees the state left by prior
/// edits. Failed edits leave content unchanged for that step;
/// subsequent edits still evaluate against the current state.
pub fn apply_edits(content: &str, edits: &[Edit]) -> (String, Vec<EditResult>) {
    let mut current = content.to_string();
    let mut results = Vec::with_capacity(edits.len());
    for edit in edits {
        let (next, result) = apply_edit(&current, edit);
        current = next;
        results.push(result);
    }
    (current, results)
}

/// Statuses that do NOT require rollback.
pub fn is_ok(status: PatchStatus) -> bool {
    matches!(status, PatchStatus::Applied | PatchStatus::AlreadyApplied)
}

// ---------- private helpers ----------

type Joiner = fn(&str, &str, &str) -> String;

fn raw_join(cb: &str, mid: &str, ca: &str) -> String {
    let mut out = String::with_capacity(cb.len() + mid.len() + ca.len());
    out.push_str(cb);
    out.push_str(mid);
    out.push_str(ca);
    out
}

fn nl_aware_join_left(cb: &str, mid: &str, ca: &str) -> String {
    // cb gets a trailing newline; ca stays raw.
    let mut out = String::with_capacity(cb.len() + 1 + mid.len() + ca.len());
    out.push_str(cb);
    out.push('\n');
    out.push_str(mid);
    out.push_str(ca);
    out
}

fn nl_aware_join_right(cb: &str, mid: &str, ca: &str) -> String {
    let mut out = String::with_capacity(cb.len() + mid.len() + 1 + ca.len());
    out.push_str(cb);
    out.push_str(mid);
    out.push('\n');
    out.push_str(ca);
    out
}

fn nl_aware_join_both(cb: &str, mid: &str, ca: &str) -> String {
    let mut out = String::with_capacity(cb.len() + 1 + mid.len() + 1 + ca.len());
    out.push_str(cb);
    out.push('\n');
    out.push_str(mid);
    out.push('\n');
    out.push_str(ca);
    out
}

/// Yield join strategies in priority order: raw first, then
/// newline-aware. Returns at most two joiners — raw, plus the single
/// newline-aware variant that actually changes anything.
///
/// Order matters: raw first means inline anchors keep their existing
/// behaviour. Only when raw yields zero matches do we try the
/// line-aware join.
fn candidate_joiners(cb: &str, mid: &str, ca: &str) -> Vec<Joiner> {
    let mut joiners: Vec<Joiner> = vec![raw_join];

    let cb_needs_nl = !cb.is_empty() && !cb.ends_with('\n') && !mid.starts_with('\n');
    let ca_needs_nl = !ca.is_empty() && !ca.starts_with('\n') && !mid.ends_with('\n');

    match (cb_needs_nl, ca_needs_nl) {
        (true, true) => joiners.push(nl_aware_join_both),
        (true, false) => joiners.push(nl_aware_join_left),
        (false, true) => joiners.push(nl_aware_join_right),
        (false, false) => {} // raw is identical to nl-aware; skip.
    }
    joiners
}

fn count_substring(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    let needle_bytes = needle.as_bytes();
    let hay_bytes = haystack.as_bytes();
    while start + needle_bytes.len() <= hay_bytes.len() {
        if &hay_bytes[start..start + needle_bytes.len()] == needle_bytes {
            count += 1;
            start += needle_bytes.len();
        } else {
            start += 1;
        }
    }
    count
}

fn replace_first(haystack: &str, needle: &str, replacement: &str) -> String {
    match haystack.find(needle) {
        Some(idx) => {
            let mut out = String::with_capacity(haystack.len() - needle.len() + replacement.len());
            out.push_str(&haystack[..idx]);
            out.push_str(replacement);
            out.push_str(&haystack[idx + needle.len()..]);
            out
        }
        None => haystack.to_string(),
    }
}

// ---------- tests: every case from tests/test_edit_engine.py ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn anchored(old: &str, new: &str, before: &str, after: &str) -> Edit {
        Edit::new(old, new).with_context(before, after)
    }

    #[test]
    fn applied_when_anchor_unique() {
        let content = "header\noriginal\nfooter\n";
        let edit = anchored("original", "renamed", "header\n", "\nfooter");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::Applied);
        assert_eq!(result.matches, 1);
        assert_eq!(new, "header\nrenamed\nfooter\n");
    }

    #[test]
    fn prefix_overlap_is_disambiguated_by_anchor() {
        // str.count would find "x = 1" inside "x = 10"; the anchor
        // must prevent this misfire.
        let content = "x = 10\n";
        let edit = anchored("x = 1", "x = 10", "", "\n");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::AlreadyApplied);
        assert_eq!(new, content);
    }

    #[test]
    fn ambiguous_when_anchor_appears_twice() {
        let content = "a\nfoo\nb\na\nfoo\nb\n";
        let edit = anchored("foo", "bar", "a\n", "\nb");
        let (_, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::Ambiguous);
        assert_eq!(result.matches, 2);
    }

    #[test]
    fn already_applied_when_anchored_new_string_present() {
        let content = "pre\nrenamed\npost\n";
        let edit = anchored("original", "renamed", "pre\n", "\npost");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::AlreadyApplied);
        assert_eq!(new, content);
    }

    #[test]
    fn not_found_when_neither_anchor_matches() {
        let content = "unrelated\n";
        let edit = anchored("missing", "replacement", "ctx\n", "\nctx");
        let (_, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::NotFound);
    }

    #[test]
    fn empty_context_falls_back_to_raw_uniqueness() {
        let content = "header\ntoken\nfooter\n";
        let edit = anchored("token", "newtoken", "", "");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::Applied);
        assert_eq!(new, "header\nnewtoken\nfooter\n");
    }

    #[test]
    fn empty_context_does_not_flag_already_applied_by_mistake() {
        // Without context, raw new_string presence must NOT claim
        // AlreadyApplied — that branch only fires through the
        // anchored path.
        let content = "renamed\n";
        let edit = anchored("original", "renamed", "", "");
        let (_, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::NotFound);
    }

    #[test]
    fn sequential_edits_see_prior_changes() {
        let content = "a = 1\nb = 2\n";
        let edits = vec![
            anchored("a = 1", "a = 10", "", "\nb"),
            anchored("b = 2", "b = 20", "a = 10\n", "\n"),
        ];
        let (final_, results) = apply_edits(content, &edits);
        assert!(
            results.iter().all(|r| r.status == PatchStatus::Applied),
            "{:?}",
            results
        );
        assert_eq!(final_, "a = 10\nb = 20\n");
    }

    #[test]
    fn is_ok_helper() {
        assert!(is_ok(PatchStatus::Applied));
        assert!(is_ok(PatchStatus::AlreadyApplied));
        assert!(!is_ok(PatchStatus::NotFound));
        assert!(!is_ok(PatchStatus::Ambiguous));
    }

    #[test]
    fn line_level_anchor_without_explicit_newlines_matches() {
        // Regression for syntax_fix: gemma-4-31b-it emitted
        // `context_after = "    return a + b"` (no leading newline)
        // for a `def add(a, b)` line; the file holds a newline
        // between the two lines. Pure byte-concat would NotFound a
        // perfectly correct plan.
        let content =
            "def add(a, b)\n    return a + b\n\n\ndef multiply(a, b):\n    return a * b\n";
        let edit = anchored("def add(a, b)", "def add(a, b):", "", "    return a + b");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::Applied);
        assert_eq!(result.matches, 1);
        assert!(new.contains("def add(a, b):\n    return a + b"));
        // The other function must be untouched.
        assert!(new.contains("def multiply(a, b):\n    return a * b"));
    }

    #[test]
    fn line_level_anchor_already_applied_via_newline_form() {
        let content = "def add(a, b):\n    return a + b\n";
        let edit = anchored("def add(a, b)", "def add(a, b):", "", "    return a + b");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::AlreadyApplied);
        assert_eq!(new, content);
    }

    #[test]
    fn inline_anchor_still_uses_raw_join() {
        // An anchor with an explicit trailing `\n` must keep its
        // inline semantics — the newline-aware fallback must NOT
        // double-add a newline.
        let content = "x = 1\n";
        let edit = anchored("x = 1", "x = 10", "", "\n");
        let (new, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::Applied);
        assert_eq!(new, "x = 10\n");
    }

    #[test]
    fn empty_old_string_is_not_found() {
        // Defensive: `old_string == ""` would otherwise count() the
        // empty needle and return Ambiguous. Python returns
        // NotFound here.
        let content = "anything";
        let edit = Edit::new("", "x");
        let (_, result) = apply_edit(content, &edit);
        assert_eq!(result.status, PatchStatus::NotFound);
        assert_eq!(result.matches, 0);
    }
}
