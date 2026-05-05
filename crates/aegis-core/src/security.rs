//! Ring 0.7 — security violations layer.
//!
//! Boolean-violation layer (not cost-regression). Each rule below
//! catches a high-frequency, shallow, pattern-matchable LLM
//! security failure where false positives are negligible.
//!
//! Discipline alignment: this layer is **only** allowed to add
//! rules satisfying:
//!   1. AST/regex pattern, no dataflow analysis
//!   2. False-positive rate < 1% on production code
//!   3. The pattern has no legitimate use we couldn't refactor
//!
//! Rules can be silenced per-line with `// aegis-allow: <rule-id>`
//! or `# aegis-allow: <rule-id>` on the same or previous line.
//!
//! ## Invariant — Message Sanitization (S4.6)
//!
//! **Violation `message` strings MUST NOT include any text extracted
//! from string literals in the analyzed source.** Only AST identifier
//! names (function names, variable names, fixed regex needles) and
//! fixed message templates are permitted as interpolation inputs.
//!
//! Reason: aegis output is consumed by upstream LLM agents. If we
//! ever embedded analyzed-code string content into a violation
//! message, we would create a prompt-injection passthrough — a
//! malicious source file could put text in a string literal that
//! then becomes instructions in the upstream agent's context.
//!
//! Adding a new rule? Audit your `format!` calls. The structured
//! payload (added by `reasons::ring0_7_security`) is the only place
//! free-form per-violation data should live, and even then only
//! identifier-derived data.

use tree_sitter::{Node, Parser};

use crate::ast::parsed_file::ParsedFile;
use crate::ast::registry::LanguageRegistry;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecurityViolation {
    pub rule_id: String,
    pub message: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: String, // "block" | "warn"
}

/// Run Ring 0.7 on a file. Returns violations not silenced by
/// `aegis-allow` comments.
pub fn check_security(filepath: &str, code: &str) -> Vec<SecurityViolation> {
    let Some(adapter) = LanguageRegistry::global().for_path(filepath) else {
        return vec![];
    };
    let mut parser = Parser::new();
    if parser.set_language(adapter.tree_sitter_language()).is_err() {
        return vec![];
    }
    let Some(tree) = parser.parse(code, None) else {
        return vec![];
    };
    let src = code.as_bytes();
    let mut out: Vec<SecurityViolation> = Vec::new();
    walk(tree.root_node(), src, &mut out);
    // Text-based rules (CORS pair, regex on whole file).
    out.extend(scan_text_rules(code));
    suppress_allowed(out, code)
}

/// Layer 1-shared variant — run Ring 0.7 against a pre-parsed
/// `ParsedFile`. No re-parse. `aegis-allow` suppression still runs
/// (the user-facing semantics don't change in this PR; that becomes
/// `user_acknowledged` annotation in PR 4).
pub fn check_security_from_parsed(parsed: &ParsedFile<'_>) -> Vec<SecurityViolation> {
    let src = parsed.source_bytes();
    let mut out: Vec<SecurityViolation> = Vec::new();
    walk(parsed.root_node(), src, &mut out);
    out.extend(scan_text_rules(parsed.source()));
    suppress_allowed(out, parsed.source())
}

fn walk(node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let kind = node.kind();

    // Rule SEC001: eval / exec / Function() with non-trivial arg.
    if matches!(
        kind,
        "call" | "call_expression" | "invocation_expression"
            | "method_invocation" | "function_call_expression"
    ) {
        if let Some(name) = call_name(node, src) {
            check_eval(&name, node, src, out);
            check_tls_off(&name, node, src, out);
            check_shell_concat(&name, node, src, out);
            check_jwt_unsafe(&name, node, src, out);
            check_insecure_deserialization(&name, node, src, out);
            check_sql_concat(&name, node, src, out);
            check_weak_crypto(&name, node, src, out);
            check_weak_random_for_token(&name, node, src, out);
        }
    }

    // Rule SEC002: hardcoded high-entropy secret in assignment to
    // a secret-shaped identifier.
    if matches!(
        kind,
        "assignment" | "assignment_expression" | "variable_declarator"
            | "let_declaration" | "let_chain" | "field_declaration"
    ) {
        check_secret_assignment(node, src, out);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, out);
    }
}

fn call_name(node: Node, src: &[u8]) -> Option<String> {
    let func = node
        .child_by_field_name("function")
        .or_else(|| node.named_child(0))?;
    let text = func.utf8_text(src).ok()?;
    Some(text.to_string())
}

// ─── Rule SEC001: eval/exec/Function ─────────────────────────────
fn check_eval(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let n = name.trim();
    let last = n.rsplit('.').next().unwrap_or(n);
    if !matches!(last, "eval" | "exec" | "Function") {
        return;
    }
    // Skip if arg is a pure string literal with no interpolation.
    let args_node = node.child_by_field_name("arguments").or_else(|| {
        let mut cursor = node.walk();
        let mut found = None;
        for child in node.children(&mut cursor) {
            if matches!(child.kind(), "arguments" | "argument_list") {
                found = Some(child);
                break;
            }
        }
        found
    });
    if let Some(args) = args_node {
        if all_args_are_safe_literals(args, src) {
            return;
        }
    }
    push(out, node, "SEC001", "block",
        format!("`{n}` invocation with non-literal/composed argument — arbitrary code execution risk"));
}

fn all_args_are_safe_literals(args: Node, src: &[u8]) -> bool {
    let mut cursor = args.walk();
    let mut any = false;
    for child in args.named_children(&mut cursor) {
        any = true;
        let kind = child.kind();
        if !matches!(kind, "string" | "string_literal" | "interpreted_string_literal") {
            return false;
        }
        if let Ok(text) = child.utf8_text(src) {
            if text.contains("${") || text.starts_with("f\"") || text.starts_with("f'") {
                return false;
            }
        }
        if has_interpolation(child) {
            return false;
        }
    }
    // Zero-arg call: there is nothing dynamic to worry about. `eval()`
    // with no args is a TypeError at runtime, not a security risk;
    // flagging it confuses the agent. Treat as "safe" (returning true
    // tells the caller to skip the violation).
    if !any {
        return true;
    }
    true
}

fn has_interpolation(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "interpolation" | "template_substitution" | "string_interpolation"
        ) {
            return true;
        }
        if has_interpolation(child) {
            return true;
        }
    }
    false
}

// ─── Rule SEC002: hardcoded secret ───────────────────────────────
fn check_secret_assignment(node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let Ok(text) = node.utf8_text(src) else { return };
    let lower = text.to_ascii_lowercase();
    let secret_name_present = ["api_key", "apikey", "api-key", "secret", "password", "token", "bearer", "private_key"]
        .iter()
        .any(|k| lower.contains(k));
    if !secret_name_present {
        return;
    }
    // Find string literal child.
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "string" | "string_literal") {
            if let Ok(s) = child.utf8_text(src) {
                let stripped = s.trim_matches(|c: char| c == '"' || c == '\'' || c == '`');
                if stripped.len() >= 20 && shannon_entropy(stripped) >= 4.0 {
                    let lower_var = lower.clone();
                    if lower_var.contains("test_") || lower_var.contains("mock_") || lower_var.contains("example_") || lower_var.contains("dummy_") {
                        return;
                    }
                    push(out, node, "SEC002", "block",
                        format!("hardcoded high-entropy secret literal in {}-named variable", "secret"));
                    return;
                }
            }
        }
    }
}

fn shannon_entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    let mut h = 0.0;
    for c in counts.iter().copied().filter(|c| *c > 0) {
        let p = c as f64 / len;
        h -= p * p.log2();
    }
    h
}

// ─── Rule SEC003: TLS verification disabled ──────────────────────
fn check_tls_off(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let _ = name;
    let Ok(text) = node.utf8_text(src) else { return };
    // Normalize: strip whitespace around '=' / ':' so spacing variants
    // collapse to a canonical form. Lowercase the value side only.
    let normalized = normalize_kv(text);
    let bad: &[(&str, &str)] = &[
        ("verify=false", "verify=False"),
        ("rejectunauthorized:false", "rejectUnauthorized: false"),
        ("insecureskipverify:true", "InsecureSkipVerify: true"),
        ("servercertificatevalidationcallback", "ServerCertificateValidationCallback"),
    ];
    for (needle, display) in bad {
        if normalized.contains(needle) {
            push(out, node, "SEC003", "block",
                format!("TLS verification disabled (`{display}`) — exposes traffic to MITM"));
            return;
        }
    }
}

/// Collapse whitespace around `=` and `:` and lowercase the whole
/// thing. `verify = False` and `verify=False` and `VERIFY=false`
/// all become `verify=false`. Used for SEC003-style invariants
/// where spacing is irrelevant but spelling is.
fn normalize_kv(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_was_space = false;
    for ch in s.chars() {
        if ch == ' ' || ch == '\t' || ch == '\n' {
            prev_was_space = true;
            continue;
        }
        if matches!(ch, '=' | ':') {
            out.push(ch);
            prev_was_space = false;
            continue;
        }
        if prev_was_space && !out.is_empty() && !matches!(out.chars().last(), Some('=' | ':')) {
            out.push(' ');
        }
        out.extend(ch.to_lowercase());
        prev_was_space = false;
    }
    out
}

// ─── Rule SEC004: shell command with interpolation ───────────────
fn check_shell_concat(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let last = name.rsplit('.').next().unwrap_or(name);
    if !matches!(last, "system" | "popen" | "call" | "run" | "Popen" | "exec" | "execSync" | "spawn") {
        return;
    }
    let Ok(text) = node.utf8_text(src) else { return };
    let has_shell_true = text.contains("shell=True") || text.contains("shell: true");
    let has_interp = text.contains("${")
        || text.contains("f\"") || text.contains("f'")
        || (text.contains("+") && text.contains("\""));
    if has_shell_true && has_interp {
        push(out, node, "SEC004", "block",
            "shell command with `shell=True` and string interpolation — command-injection risk".into());
    }
}

// ─── Rule SEC005: SQL string concat ──────────────────────────────
fn check_sql_concat(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let last = name.rsplit('.').next().unwrap_or(name);
    if !matches!(last, "execute" | "executemany" | "query" | "executeQuery" | "executeUpdate") {
        return;
    }
    let Ok(text) = node.utf8_text(src) else { return };

    // Skip ORM builder patterns: SQLAlchemy's `select(User).where(...)`,
    // Django's `Model.objects.filter(...)`. These call `.execute(stmt)`
    // where `stmt` is a builder object — no SQL string at the call site
    // → safe. We detect this by looking for the SQL keyword in a
    // string-literal context, not just anywhere in the call text.
    if !contains_sql_in_string_literal(node, src) {
        return;
    }

    let has_interp = text.contains("${")
        || text.contains("f\"") || text.contains("f'")
        || text.contains(".format(")
        || text.contains(" + ")
        || text.contains(" % ");
    if has_interp {
        push(out, node, "SEC005", "warn",
            "SQL query with string interpolation/concat — use parameterized queries".into());
    }
}

/// Recursively check whether any string literal under `node` contains
/// SQL keywords. Guards SEC005 against `Model.objects.execute(select(...))`
/// where `select(...)` is a builder, not a string.
fn contains_sql_in_string_literal(node: Node, src: &[u8]) -> bool {
    let kind = node.kind();
    if matches!(kind, "string" | "string_literal" | "interpreted_string_literal" | "string_fragment") {
        if let Ok(text) = node.utf8_text(src) {
            let upper = text.to_ascii_uppercase();
            return ["SELECT ", "INSERT ", "UPDATE ", "DELETE ", "DROP "]
                .iter()
                .any(|k| upper.contains(k));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if contains_sql_in_string_literal(child, src) {
            return true;
        }
    }
    false
}

// ─── Rule SEC007: JWT decoded without verification ───────────────
fn check_jwt_unsafe(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let last = name.rsplit('.').next().unwrap_or(name);
    let Ok(text) = node.utf8_text(src) else { return };
    if last == "decode" && (name.contains("jwt") || name.contains("JWT")) {
        if text.contains("verify=False") || text.contains("\"verify\": false") || text.contains("'verify': false") {
            push(out, node, "SEC007", "block",
                "JWT decoded with `verify=False` — accepts forged tokens".into());
            return;
        }
        if !text.contains("algorithms") && !text.contains("key") && !text.contains("verify") {
            push(out, node, "SEC007", "block",
                "JWT `decode()` without verification kwargs — accepts forged tokens".into());
            return;
        }
    }
    if text.contains("algorithms=['none']")
        || text.contains("algorithms=[\"none\"]")
        || text.contains("algorithms: ['none']")
        || text.contains("algorithms: [\"none\"]")
    {
        push(out, node, "SEC007", "block",
            "JWT `algorithms` list contains 'none' — disables signature verification".into());
    }
}

// ─── Rule SEC008: insecure deserialization ───────────────────────
fn check_insecure_deserialization(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let dangerous = [
        "pickle.loads", "pickle.load",
        "marshal.loads", "marshal.load",
        "yaml.load",
        "node-serialize.unserialize",
        "ObjectInputStream", "readObject",
    ];
    if !dangerous.iter().any(|n| name.contains(n)) {
        return;
    }
    let Ok(text) = node.utf8_text(src) else { return };
    // Special-case: yaml.load with SafeLoader is safe.
    if name.contains("yaml.load") && (text.contains("SafeLoader") || text.contains("safe_load")) {
        return;
    }
    // Skip if arg is a literal (test fixture pattern).
    let args = node.child_by_field_name("arguments");
    if let Some(args) = args {
        if all_args_are_safe_literals(args, src) {
            return;
        }
    }
    push(out, node, "SEC008", "block",
        format!("insecure deserialization via `{name}` with non-literal input"));
}

// ─── Rule SEC009: weak crypto for security context ───────────────
fn check_weak_crypto(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let last = name.rsplit('.').next().unwrap_or(name);
    // Common hash entry points across languages.
    let weak = matches!(
        last,
        "md5" | "sha1" | "MD5" | "SHA1" | "createHash"
    ) || name.ends_with(".md5")
        || name.ends_with(".sha1")
        || name.ends_with("hashlib.md5")
        || name.ends_with("hashlib.sha1");
    if !weak {
        return;
    }
    // Look at the *enclosing assignment* for a security-context
    // identifier. Walk up a few parents.
    if let Some(ctx) = enclosing_security_context(node, src) {
        push(
            out,
            node,
            "SEC009",
            "block",
            format!(
                "weak hash (md5/sha1) used in security context (`{}`) — use sha256+",
                ctx
            ),
        );
    }
}

fn enclosing_security_context(node: Node, src: &[u8]) -> Option<&'static str> {
    let mut cur = node.parent();
    // Deliberately conservative: only identifiers that strongly
    // imply a security context. Avoid generic words like "digest"
    // (matches hashlib's `.hexdigest()` method on an etag), or
    // "key" (matches dictionary keys in unrelated contexts).
    let names: &[(&str, &str)] = &[
        ("password", "password"),
        ("passwd", "password"),
        ("signature", "signature"),
        ("hmac", "hmac"),
        ("token", "token"),
        ("secret", "secret"),
    ];
    for _ in 0..6 {
        let Some(n) = cur else { break };
        if matches!(
            n.kind(),
            "assignment" | "assignment_expression" | "variable_declarator"
                | "lexical_declaration" | "let_declaration"
        ) {
            if let Ok(text) = n.utf8_text(src) {
                let lower = text.to_ascii_lowercase();
                for (needle, label) in names {
                    if lower.contains(needle) {
                        return Some(label);
                    }
                }
            }
        }
        cur = n.parent();
    }
    None
}

// ─── Rule SEC010: weak randomness for tokens/nonces ──────────────
fn check_weak_random_for_token(name: &str, node: Node, src: &[u8], out: &mut Vec<SecurityViolation>) {
    let last = name.rsplit('.').next().unwrap_or(name);
    let is_weak_rng = matches!(
        last,
        "random" | "randint" | "choice" | "uniform" | "randrange" | "shuffle"
    ) || name.ends_with("Math.random")
        || name == "Math.random";
    if !is_weak_rng {
        return;
    }
    if enclosing_token_context(node, src).is_some() {
        push(
            out,
            node,
            "SEC010",
            "block",
            "weak RNG (random.* / Math.random) used for security token — use secrets / crypto.randomBytes / crypto.getRandomValues".into(),
        );
    }
}

fn enclosing_token_context(node: Node, src: &[u8]) -> Option<&'static str> {
    let mut cur = node.parent();
    let needles = ["token", "nonce", "csrf", "otp", "session_id", "reset_code", "api_key"];
    for _ in 0..6 {
        let Some(n) = cur else { break };
        if matches!(
            n.kind(),
            "assignment" | "assignment_expression" | "variable_declarator"
                | "lexical_declaration" | "return_statement"
        ) {
            if let Ok(text) = n.utf8_text(src) {
                let lower = text.to_ascii_lowercase();
                for needle in needles {
                    if lower.contains(needle) {
                        return Some(needle);
                    }
                }
            }
        }
        cur = n.parent();
    }
    None
}

// ─── Text-level rule(s) ──────────────────────────────────────────
fn scan_text_rules(code: &str) -> Vec<SecurityViolation> {
    let mut out = Vec::new();
    let wildcard_needles = [
        "Access-Control-Allow-Origin: *",
        "Access-Control-Allow-Origin\": \"*\"",
        "origin: \"*\"",
        "origin: '*'",
        "origin: true",
    ];
    let credentials_needles = [
        "Access-Control-Allow-Credentials: true",
        "Access-Control-Allow-Credentials\": true",
        "credentials: true",
    ];
    let mut wildcard_line: Option<usize> = None;
    let mut credentials_line: Option<usize> = None;
    for (idx, line) in code.lines().enumerate() {
        if wildcard_line.is_none() && wildcard_needles.iter().any(|n| line.contains(n)) {
            wildcard_line = Some(idx + 1);
        }
        if credentials_line.is_none() && credentials_needles.iter().any(|n| line.contains(n)) {
            credentials_line = Some(idx + 1);
        }
    }
    if let (Some(w), Some(c)) = (wildcard_line, credentials_line) {
        // Anchor the violation at the first wildcard line so that the
        // suppression scanner (which only checks the violation line and
        // the line above) can actually see `// aegis-allow: SEC006`.
        let line = w.min(c);
        out.push(SecurityViolation {
            rule_id: "SEC006".into(),
            message: "CORS configured with wildcard origin AND credentials=true — spec violation, browser will reject; exposes auth surface".into(),
            start_line: line,
            start_col: 1,
            end_line: line,
            end_col: 1,
            severity: "block".into(),
        });
    }
    out
}

// ─── allow-comment suppression ───────────────────────────────────
fn suppress_allowed(violations: Vec<SecurityViolation>, code: &str) -> Vec<SecurityViolation> {
    let lines: Vec<&str> = code.lines().collect();
    violations
        .into_iter()
        .filter(|v| !is_silenced(v, &lines))
        .collect()
}

fn is_silenced(v: &SecurityViolation, lines: &[&str]) -> bool {
    let needle_specific = format!("aegis-allow: {}", v.rule_id);
    let needle_all = "aegis-allow: all";
    let line_idx = v.start_line.saturating_sub(1);
    if let Some(line) = lines.get(line_idx) {
        if line.contains(&needle_specific) || line.contains(needle_all) {
            return true;
        }
    }
    if line_idx > 0 {
        if let Some(prev) = lines.get(line_idx - 1) {
            if prev.contains(&needle_specific) || prev.contains(needle_all) {
                return true;
            }
        }
    }
    false
}

fn push(out: &mut Vec<SecurityViolation>, node: Node, rule_id: &str, severity: &str, message: String) {
    let s = node.start_position();
    let e = node.end_position();
    out.push(SecurityViolation {
        rule_id: rule_id.into(),
        message,
        start_line: s.row + 1,
        start_col: s.column + 1,
        end_line: e.row + 1,
        end_col: e.column + 1,
        severity: severity.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn check(suffix: &str, code: &str) -> Vec<SecurityViolation> {
        let mut tmp = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        tmp.write_all(code.as_bytes()).unwrap();
        tmp.flush().unwrap();
        check_security(tmp.path().to_str().unwrap(), code)
    }

    #[test]
    fn sec001_eval_with_dynamic_arg_blocks() {
        let v = check(".py", "def f(user_input):\n    eval(user_input)\n");
        assert!(v.iter().any(|v| v.rule_id == "SEC001"), "got {v:?}");
    }

    #[test]
    fn sec001_eval_with_string_literal_passes() {
        let v = check(".py", "eval(\"1 + 1\")\n");
        assert!(!v.iter().any(|v| v.rule_id == "SEC001"), "got {v:?}");
    }

    #[test]
    fn sec002_hardcoded_secret_blocks() {
        let v = check(".py", "API_KEY = \"abcdef0123456789ghijklmnopqrstuv\"\n");
        assert!(v.iter().any(|v| v.rule_id == "SEC002"), "got {v:?}");
    }

    #[test]
    fn sec002_test_var_skipped() {
        let v = check(".py", "TEST_API_KEY = \"abcdef0123456789ghijklmnopqrstuv\"\n");
        assert!(!v.iter().any(|v| v.rule_id == "SEC002"), "got {v:?}");
    }

    #[test]
    fn sec003_tls_off_blocks() {
        let v = check(".py", "import requests\nrequests.get(url, verify=False)\n");
        assert!(v.iter().any(|v| v.rule_id == "SEC003"), "got {v:?}");
    }

    #[test]
    fn sec004_shell_injection_blocks() {
        let v = check(
            ".py",
            "import subprocess\nsubprocess.run(f\"ls {user_dir}\", shell=True)\n",
        );
        assert!(v.iter().any(|v| v.rule_id == "SEC004"), "got {v:?}");
    }

    #[test]
    fn sec005_sql_concat_warns() {
        let v = check(
            ".py",
            "cursor.execute(\"SELECT * FROM users WHERE id = \" + user_id)\n",
        );
        assert!(v.iter().any(|v| v.rule_id == "SEC005"), "got {v:?}");
    }

    #[test]
    fn sec006_cors_wildcard_with_credentials_blocks() {
        let v = check(
            ".js",
            "app.use(cors({ origin: \"*\", credentials: true }));\n",
        );
        assert!(v.iter().any(|v| v.rule_id == "SEC006"), "got {v:?}");
    }

    #[test]
    fn sec007_jwt_algorithms_none_blocks() {
        let v = check(".py", "import jwt\nclaims = jwt.decode(token, key, algorithms=['none'])\n");
        assert!(v.iter().any(|v| v.rule_id == "SEC007"), "got {v:?}");
    }

    #[test]
    fn sec008_pickle_loads_blocks() {
        let v = check(".py", "import pickle\ndata = pickle.loads(payload)\n");
        assert!(v.iter().any(|v| v.rule_id == "SEC008"), "got {v:?}");
    }

    #[test]
    fn sec008_yaml_safe_load_passes() {
        let v = check(".py", "import yaml\ndata = yaml.load(content, Loader=yaml.SafeLoader)\n");
        assert!(!v.iter().any(|v| v.rule_id == "SEC008"), "got {v:?}");
    }

    #[test]
    fn sec001_eval_with_zero_args_does_not_block() {
        // S1.1: previously eval() (no args) was BLOCK — false positive.
        let v = check(".py", "eval()\n");
        assert!(!v.iter().any(|v| v.rule_id == "SEC001"), "got {v:?}");
    }

    #[test]
    fn sec003_handles_spacing_and_case_variants() {
        // S1.2: verify=False, verify = False, VERIFY=false all should fire.
        for code in [
            "requests.get(url, verify = False)\n",
            "requests.get(url, verify=false)\n",
            "requests.get(url, VERIFY=False)\n",
        ] {
            let v = check(".py", code);
            assert!(v.iter().any(|v| v.rule_id == "SEC003"),
                "expected SEC003 for {code:?}, got {v:?}");
        }
    }

    #[test]
    fn sec005_orm_select_builder_does_not_warn() {
        // S1.3: SQLAlchemy `session.execute(select(User))` is safe —
        // select() is a builder, not a SQL string.
        let v = check(".py", "session.execute(select(User).where(id == 1))\n");
        assert!(!v.iter().any(|v| v.rule_id == "SEC005"), "got {v:?}");
    }

    #[test]
    fn sec006_anchors_on_real_line_so_aegis_allow_works() {
        // S1.7: SEC006 line was hardcoded to 1, so // aegis-allow on
        // the actual CORS config line had no effect. Now anchored.
        let code = "const config = {\n  origin: \"*\",  // aegis-allow: SEC006\n  credentials: true,\n};\n";
        let v = check(".js", code);
        assert!(!v.iter().any(|v| v.rule_id == "SEC006"),
            "expected SEC006 silenced via aegis-allow, got {v:?}");
    }

    #[test]
    fn sec009_md5_for_password_blocks() {
        let v = check(
            ".py",
            "import hashlib\npassword_hash = hashlib.md5(pw.encode()).hexdigest()\n",
        );
        assert!(v.iter().any(|v| v.rule_id == "SEC009"), "got {v:?}");
    }

    #[test]
    fn sec009_md5_for_etag_does_not_block() {
        // md5 for non-security context (etag/dedup) is a legit use.
        let v = check(
            ".py",
            "import hashlib\netag = hashlib.md5(content).hexdigest()\n",
        );
        assert!(!v.iter().any(|v| v.rule_id == "SEC009"), "got {v:?}");
    }

    #[test]
    fn sec010_random_for_token_blocks() {
        let v = check(
            ".py",
            "import random\nreset_token = ''.join(random.choice(chars) for _ in range(32))\n",
        );
        assert!(v.iter().any(|v| v.rule_id == "SEC010"), "got {v:?}");
    }

    #[test]
    fn sec010_random_for_game_state_does_not_block() {
        let v = check(".py", "import random\ndice_roll = random.randint(1, 6)\n");
        assert!(!v.iter().any(|v| v.rule_id == "SEC010"), "got {v:?}");
    }

    #[test]
    fn aegis_allow_silences_specific_rule() {
        let v = check(
            ".py",
            "import requests  # aegis-allow: SEC003\nrequests.get(url, verify=False)  # aegis-allow: SEC003\n",
        );
        assert!(!v.iter().any(|v| v.rule_id == "SEC003"), "got {v:?}");
    }
}
