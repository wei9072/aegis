//! S6 — Post-write attestation.
//!
//! `validate_change` is a pre-write gate: it judges proposed content
//! before disk is touched. That contract breaks whenever an upstream
//! agent uses a different write tool, races between pre-write check
//! and actual write, or simply edits the file by hand.
//!
//! Attestation is the dual: read what's actually on disk now, run
//! every absolute (non-delta) check we can, and emit a hash-stamped
//! verdict. Designed to be called from PostToolUse hooks, CI, or
//! `aegis attest` ad hoc.
//!
//! Discipline: attestation is **observation only** — it does not
//! roll back, does not propose fixes, does not retry. Rollback
//! remains the executor's responsibility. Aegis-core just states
//! the truth at a point in time.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::ast::registry::LanguageRegistry;
use crate::enforcement::check_syntax_native_detailed;
use crate::security::check_security;
use crate::workspace::WorkspaceIndex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationVerdict {
    pub schema_version: String,
    pub path: String,
    pub content_sha256: String,
    pub timestamp_unix: u64,
    /// "PASS" — file passes all absolute checks
    /// "BLOCK" — at least one absolute check failed
    /// "SKIP" — file type unsupported (no opinion)
    /// "MISSING" — file does not exist on disk
    pub decision: String,
    pub reasons: Vec<serde_json::Value>,
}

pub const ATTESTATION_SCHEMA_VERSION: &str = "1";

/// Read `path` from disk, run absolute checks (Ring 0 syntax,
/// Ring 0.7 security, optionally Ring R2 cycle if `workspace_root`
/// is provided), and emit an attestation. Does not perform delta
/// comparisons — there is no `old_content`; this is the truth as of
/// the call.
pub fn attest(path: &str, workspace_root: Option<&str>) -> AttestationVerdict {
    let p = Path::new(path);
    let mut reasons: Vec<serde_json::Value> = Vec::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if !p.exists() {
        return AttestationVerdict {
            schema_version: ATTESTATION_SCHEMA_VERSION.into(),
            path: path.to_string(),
            content_sha256: "".into(),
            timestamp_unix: now,
            decision: "MISSING".into(),
            reasons: vec![serde_json::json!({
                "layer": "meta",
                "ring": "meta",
                "verdict_model": "not_applicable",
                "decision": "block",
                "reason": "file_missing",
                "rule_id": "file_missing",
                "detail": format!("attestation target {path:?} does not exist on disk"),
            })],
        };
    }

    let content = match std::fs::read_to_string(p) {
        Ok(c) => c,
        Err(e) => {
            return AttestationVerdict {
                schema_version: ATTESTATION_SCHEMA_VERSION.into(),
                path: path.to_string(),
                content_sha256: "".into(),
                timestamp_unix: now,
                decision: "BLOCK".into(),
                reasons: vec![serde_json::json!({
                    "layer": "meta",
                    "ring": "meta",
                    "verdict_model": "not_applicable",
                    "decision": "block",
                    "reason": "read_failed",
                    "rule_id": "read_failed",
                    "detail": e.to_string(),
                })],
            };
        }
    };

    let hash = sha256_hex(content.as_bytes());

    // Unsupported file type → SKIP (no opinion). Cohorts well with
    // the SKIP semantics in validate_change.
    let suffix = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    if !LanguageRegistry::global()
        .extensions()
        .contains(&suffix.as_str())
    {
        return AttestationVerdict {
            schema_version: ATTESTATION_SCHEMA_VERSION.into(),
            path: path.to_string(),
            content_sha256: hash,
            timestamp_unix: now,
            decision: "SKIP".into(),
            reasons: vec![serde_json::json!({
                "layer": "ring0",
                "ring": "ring0",
                "verdict_model": "not_applicable",
                "decision": "skip",
                "reason": "unsupported_extension",
                "rule_id": "unsupported_extension",
                "detail": format!("aegis has no opinion on {suffix:?}"),
            })],
        };
    }

    // Ring 0 — syntax errors on the on-disk content.
    if let Ok(violations) = check_syntax_native_detailed(path) {
        for v in violations {
            reasons.push(crate::reasons::ring0_violation(
                v.message,
                crate::reasons::Range {
                    start_line: v.start_line,
                    start_col: v.start_col,
                    end_line: v.end_line,
                    end_col: v.end_col,
                },
                &v.kind,
            ));
        }
    }

    // Ring 0.7 — absolute security violations.
    for sv in check_security(path, &content) {
        reasons.push(crate::reasons::ring0_7_security(
            &sv.rule_id,
            &sv.severity,
            sv.message,
            crate::reasons::Range {
                start_line: sv.start_line,
                start_col: sv.start_col,
                end_line: sv.end_line,
                end_col: sv.end_col,
            },
        ));
    }

    // Ring R2 — cycle detection (workspace-wide). Only the cycle
    // half is absolute; public_symbol_removed is delta-only and
    // therefore not part of attestation.
    if let Some(root) = workspace_root {
        let root_path = Path::new(root);
        if root_path.is_dir() {
            let idx = WorkspaceIndex::build_cached(root_path);
            let cycle = idx.find_cycle();
            if !cycle.is_empty() && cycle.iter().any(|f| f == path) {
                reasons.push(crate::reasons::ringR2_cycle_introduced(cycle, path));
            }
        }
    }

    let any_block = reasons
        .iter()
        .any(|r| r.get("decision").and_then(|d| d.as_str()) == Some("block"));
    let decision = if any_block { "BLOCK" } else { "PASS" };

    AttestationVerdict {
        schema_version: ATTESTATION_SCHEMA_VERSION.into(),
        path: path.to_string(),
        content_sha256: hash,
        timestamp_unix: now,
        decision: decision.into(),
        reasons,
    }
}

/// Append the attestation as one JSONL row to
/// `<workspace_root>/.aegis/attestations.jsonl`. Builds the directory
/// if missing. Returns Err on IO failure but doesn't propagate up
/// to the verdict — attestation logging is best-effort.
pub fn append_attestation_log(
    workspace_root: &str,
    verdict: &AttestationVerdict,
) -> std::io::Result<()> {
    use std::io::Write;
    let dir = Path::new(workspace_root).join(".aegis");
    std::fs::create_dir_all(&dir)?;
    let log_path = dir.join("attestations.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let line = serde_json::to_string(verdict).unwrap_or_default();
    writeln!(f, "{line}")?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let mut s = String::with_capacity(result.len() * 2);
    for b in result {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(suffix: &str, body: &str) -> tempfile::NamedTempFile {
        let mut tmp = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        tmp.write_all(body.as_bytes()).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn attest_pass_on_clean_python() {
        let tmp = write_tmp(".py", "def add(a, b):\n    return a + b\n");
        let v = attest(tmp.path().to_str().unwrap(), None);
        assert_eq!(v.decision, "PASS");
        assert!(!v.content_sha256.is_empty());
    }

    #[test]
    fn attest_block_on_syntax_error() {
        let tmp = write_tmp(".py", "def f(\n");
        let v = attest(tmp.path().to_str().unwrap(), None);
        assert_eq!(v.decision, "BLOCK");
        assert!(v.reasons.iter().any(|r| r["ring"] == "ring0"));
    }

    #[test]
    fn attest_block_on_security_violation() {
        let tmp = write_tmp(
            ".py",
            "import requests\nrequests.get(url, verify=False)\n",
        );
        let v = attest(tmp.path().to_str().unwrap(), None);
        assert_eq!(v.decision, "BLOCK");
        assert!(v.reasons.iter().any(|r| r["rule_id"] == "SEC003"));
    }

    #[test]
    fn attest_skip_on_unsupported() {
        let tmp = write_tmp(".xyz", "anything");
        let v = attest(tmp.path().to_str().unwrap(), None);
        assert_eq!(v.decision, "SKIP");
    }

    #[test]
    fn attest_missing_file() {
        let v = attest("/nonexistent/path/foo.py", None);
        assert_eq!(v.decision, "MISSING");
    }

    #[test]
    fn attestation_log_append_creates_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let v = AttestationVerdict {
            schema_version: ATTESTATION_SCHEMA_VERSION.into(),
            path: "x.py".into(),
            content_sha256: "abc".into(),
            timestamp_unix: 0,
            decision: "PASS".into(),
            reasons: vec![],
        };
        append_attestation_log(dir.path().to_str().unwrap(), &v).unwrap();
        let log = std::fs::read_to_string(dir.path().join(".aegis/attestations.jsonl")).unwrap();
        assert!(log.contains("\"decision\":\"PASS\""));
        // Append second
        append_attestation_log(dir.path().to_str().unwrap(), &v).unwrap();
        let log2 = std::fs::read_to_string(dir.path().join(".aegis/attestations.jsonl")).unwrap();
        assert_eq!(log2.lines().count(), 2);
    }

    #[test]
    fn content_hash_is_stable() {
        let tmp = write_tmp(".py", "x = 1\n");
        let v1 = attest(tmp.path().to_str().unwrap(), None);
        let v2 = attest(tmp.path().to_str().unwrap(), None);
        assert_eq!(v1.content_sha256, v2.content_sha256);
        assert!(!v1.content_sha256.is_empty());
    }
}
