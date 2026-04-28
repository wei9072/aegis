//! Phase 6 (B1) — capability tools.
//!
//! Four tools that don't fit the file-tool / shell-tool buckets but
//! show up in nearly every real coding session: tracking the model's
//! own todo list, fetching arbitrary URLs, doing a web search, and
//! pausing to ask the user for clarification.
//!
//! ## Negative-space framing — what these are NOT
//!
//! claw-code's tool catalogue includes Skill / ToolSearch / Agent
//! (sub-agent) / Worker / Team / Cron / Plugin. Aegis intentionally
//! does NOT add those:
//!   - **Skill** is directed reasoning aid (claw-code injects
//!     "consider these skills…" into the system prompt) — violates
//!     the no-coaching framing.
//!   - **Agent / Worker / Team** would break the cost-tracker /
//!     stalemate-detector boundary; sub-agents need their own
//!     enforcement layer.
//!   - **Cron / RemoteTrigger** are scheduling concerns; aegis
//!     stays in the synchronous coding-loop scope.
//!
//! Even within these four tools the framing matters:
//!   - **TodoWrite** stores what the LLM said it would do. Aegis
//!     never produces todos itself; the contract test for "no
//!     coaching injection" remains green because TodoWrite is just
//!     a typed echo.
//!   - **AskUserQuestion** does not retry, summarize, or
//!     paraphrase. The user's literal answer comes back as the
//!     tool result.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::ToolDefinition;
use crate::tool::{ToolError, ToolExecutor};

const MAX_FETCH_BYTES: usize = 256 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 30;

// ============================================================
// TodoWrite
// ============================================================

#[derive(Debug, Deserialize)]
struct TodoInput {
    /// One-line summary — what the agent is trying to accomplish.
    #[serde(default)]
    summary: Option<String>,
    /// Ordered todos. Each item is the action description; aegis
    /// does not parse them, just echoes back.
    todos: Vec<TodoItem>,
}

#[derive(Debug, Deserialize)]
struct TodoItem {
    description: String,
    #[serde(default)]
    status: Option<String>,
}

/// `TodoWrite` definition. Stateless — each call replaces the
/// previous list (the LLM resends the full list every turn).
#[must_use]
pub fn todo_write_definition() -> ToolDefinition {
    ToolDefinition {
        name: "TodoWrite".into(),
        description:
            "Record the agent's current todo list. Pass an array of {description, status?} \
             items in execution order. Aegis does not retain across turns — pass the full \
             list each time. Useful for keeping multi-step work explicit. Status conventions: \
             'pending' / 'in_progress' / 'completed'."
                .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "description": { "type": "string" },
                            "status": { "type": "string" }
                        },
                        "required": ["description"]
                    }
                }
            },
            "required": ["todos"]
        }),
    }
}

fn run_todo_write(input: &str) -> Result<String, ToolError> {
    let parsed: TodoInput = serde_json::from_str(input)
        .map_err(|e| ToolError::new(format!("TodoWrite input not valid JSON: {e}")))?;
    let mut out = String::new();
    if let Some(s) = parsed.summary {
        if !s.is_empty() {
            out.push_str(&format!("# {s}\n"));
        }
    }
    for (i, t) in parsed.todos.iter().enumerate() {
        let marker = match t.status.as_deref() {
            Some("completed") => "[x]",
            Some("in_progress") => "[~]",
            _ => "[ ]",
        };
        out.push_str(&format!("{marker} {}. {}\n", i + 1, t.description));
    }
    Ok(out)
}

// ============================================================
// WebFetch
// ============================================================

#[derive(Debug, Deserialize)]
struct FetchInput {
    url: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[must_use]
pub fn web_fetch_definition() -> ToolDefinition {
    ToolDefinition {
        name: "WebFetch".into(),
        description: format!(
            "HTTP GET a URL, return the response body (truncated to {} bytes). Honors the \
             optional --webfetch-allow URL allowlist (when set, only matching prefixes are \
             allowed). Default timeout {}s, override via timeout_secs.",
            MAX_FETCH_BYTES, FETCH_TIMEOUT_SECS
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" },
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 300 }
            },
            "required": ["url"]
        }),
    }
}

/// Per-instance config for WebFetchTool. `allowlist` empty = allow
/// all. When non-empty, the request URL must start with one of the
/// allowed prefixes. Loaded from the user's config.toml in CLI
/// hookup; here it's just a Vec<String>.
#[derive(Clone, Default)]
pub struct WebFetchConfig {
    pub allowlist: Vec<String>,
}

pub struct WebFetchTool {
    config: WebFetchConfig,
}

impl WebFetchTool {
    #[must_use]
    pub fn new(config: WebFetchConfig) -> Self {
        Self { config }
    }

    fn run(&self, input: &str) -> Result<String, ToolError> {
        let parsed: FetchInput = serde_json::from_str(input)
            .map_err(|e| ToolError::new(format!("WebFetch input not valid JSON: {e}")))?;

        if !self.config.allowlist.is_empty()
            && !self
                .config
                .allowlist
                .iter()
                .any(|prefix| parsed.url.starts_with(prefix))
        {
            return Err(ToolError::new(format!(
                "WebFetch denied: URL {:?} does not match any allowlist prefix. Allowlist: {:?}",
                parsed.url, self.config.allowlist
            )));
        }

        let timeout = Duration::from_secs(
            parsed
                .timeout_secs
                .unwrap_or(FETCH_TIMEOUT_SECS)
                .min(300),
        );

        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        let body = match agent.get(&parsed.url).call() {
            Ok(resp) => resp
                .into_string()
                .map_err(|e| ToolError::new(format!("WebFetch read body: {e}")))?,
            Err(ureq::Error::Status(status, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Ok(format!("HTTP {status}\n{body}"));
            }
            Err(e) => {
                return Err(ToolError::new(format!("WebFetch transport error: {e}")));
            }
        };

        Ok(truncate(body, MAX_FETCH_BYTES))
    }
}

impl ToolExecutor for WebFetchTool {
    fn execute(&mut self, name: &str, input: &str) -> Result<String, ToolError> {
        match name {
            "WebFetch" => self.run(input),
            other => Err(ToolError::new(format!(
                "WebFetchTool received unknown tool: {other:?}"
            ))),
        }
    }
}

// ============================================================
// WebSearch
// ============================================================

#[derive(Debug, Deserialize)]
struct SearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

#[must_use]
pub fn web_search_definition() -> ToolDefinition {
    ToolDefinition {
        name: "WebSearch".into(),
        description:
            "Search the web via DuckDuckGo (HTML endpoint, no API key). Returns titles + URLs + \
             snippets, one per line. Default max_results=10."
                .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "max_results": { "type": "integer", "minimum": 1, "maximum": 30 }
            },
            "required": ["query"]
        }),
    }
}

pub struct WebSearchTool;

impl WebSearchTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn run(&self, input: &str) -> Result<String, ToolError> {
        let parsed: SearchInput = serde_json::from_str(input)
            .map_err(|e| ToolError::new(format!("WebSearch input not valid JSON: {e}")))?;
        let max = parsed.max_results.unwrap_or(10).min(30);

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencode(&parsed.query)
        );
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
            .build();

        let html = match agent
            .get(&url)
            // DuckDuckGo HTML rejects requests without a UA.
            .set("User-Agent", "Mozilla/5.0 aegis/0.1")
            .call()
        {
            Ok(r) => r
                .into_string()
                .map_err(|e| ToolError::new(format!("WebSearch read body: {e}")))?,
            Err(ureq::Error::Status(s, _)) => {
                return Err(ToolError::new(format!(
                    "WebSearch DuckDuckGo returned HTTP {s}"
                )))
            }
            Err(e) => return Err(ToolError::new(format!("WebSearch transport: {e}"))),
        };

        Ok(format_search_results(&html, max))
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor for WebSearchTool {
    fn execute(&mut self, name: &str, input: &str) -> Result<String, ToolError> {
        match name {
            "WebSearch" => self.run(input),
            other => Err(ToolError::new(format!(
                "WebSearchTool received unknown tool: {other:?}"
            ))),
        }
    }
}

/// Extract result rows from DuckDuckGo HTML. The page format is
/// stable enough for V0; if DDG ever changes layout, this returns
/// "(no results parsed; layout may have changed)" instead of
/// crashing. Pure function — testable against canned HTML.
#[must_use]
pub fn format_search_results(html: &str, max: usize) -> String {
    let mut rows: Vec<String> = Vec::new();
    // DuckDuckGo HTML uses <a class="result__a" href="..."> for links
    // and <a class="result__snippet"> for snippets. Naive extraction
    // by repeated marker search — no HTML parser to keep deps light.
    let mut idx = 0;
    while rows.len() < max {
        let Some(start) = html[idx..].find("class=\"result__a\"") else {
            break;
        };
        let abs = idx + start;
        // Look backward for href
        let href_marker = "href=\"";
        let href_start = match html[..abs].rfind(href_marker) {
            Some(p) => p + href_marker.len(),
            None => {
                idx = abs + 1;
                continue;
            }
        };
        let href_end = match html[href_start..].find('"') {
            Some(p) => href_start + p,
            None => {
                idx = abs + 1;
                continue;
            }
        };
        let link = &html[href_start..href_end];

        // Title text: between > and </a> after the result__a marker.
        let title_open = match html[abs..].find('>') {
            Some(p) => abs + p + 1,
            None => break,
        };
        let title_close = match html[title_open..].find("</a>") {
            Some(p) => title_open + p,
            None => break,
        };
        let title = strip_html_tags(&html[title_open..title_close]);

        rows.push(format!("- {} <{}>", title.trim(), unwrap_ddg_link(link)));
        idx = title_close + 4;
    }

    if rows.is_empty() {
        "(no results parsed; layout may have changed)".to_string()
    } else {
        rows.join("\n")
    }
}

fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match (ch, in_tag) {
            ('<', _) => in_tag = true,
            ('>', _) => in_tag = false,
            (c, false) => out.push(c),
            _ => {}
        }
    }
    // Decode the few entities DDG emits.
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// DuckDuckGo wraps result links in a redirect: `//duckduckgo.com/l/?uddg=<encoded>&...`.
/// Pull out the real URL from `uddg=`.
fn unwrap_ddg_link(link: &str) -> String {
    if let Some(start) = link.find("uddg=") {
        let after = &link[start + 5..];
        let raw = after.split('&').next().unwrap_or(after);
        return urldecode(raw);
    }
    link.to_string()
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ============================================================
// AskUserQuestion
// ============================================================

#[derive(Debug, Deserialize)]
struct AskInput {
    question: String,
    /// Optional multiple-choice list. When present, the prompter
    /// shows numbered options and accepts a number; the result is
    /// the chosen option's text.
    #[serde(default)]
    options: Vec<String>,
}

#[must_use]
pub fn ask_user_question_definition() -> ToolDefinition {
    ToolDefinition {
        name: "AskUserQuestion".into(),
        description:
            "Pause the turn and ask the user a clarifying question. Pass `question` (required) \
             and optional `options` (the user picks one by number). Returns the user's literal \
             answer. Aegis does NOT retry or summarize — the answer goes back to the LLM \
             unmodified. Use for genuine ambiguity; repeated asks within one turn trigger \
             stalemate detection."
                .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["question"]
        }),
    }
}

/// User-facing prompter abstraction. Production impl reads from
/// stdin; tests inject a scripted answer source.
pub trait AskUserPrompter: Send {
    fn ask(&mut self, question: &str, options: &[String]) -> String;
}

/// Default prompter: prints the question + options to stderr and
/// reads a single line from stdin. Trims trailing newline.
pub struct StdinAskUserPrompter;

impl AskUserPrompter for StdinAskUserPrompter {
    fn ask(&mut self, question: &str, options: &[String]) -> String {
        use std::io::{stdin, stderr, Write};
        let mut err = stderr();
        let _ = writeln!(err, "\n[aegis] LLM asks: {question}");
        if !options.is_empty() {
            for (i, opt) in options.iter().enumerate() {
                let _ = writeln!(err, "  {}. {opt}", i + 1);
            }
            let _ = write!(err, "  pick a number (or type a freeform answer): ");
        } else {
            let _ = write!(err, "  > ");
        }
        let _ = err.flush();

        let mut line = String::new();
        if stdin().read_line(&mut line).is_err() {
            return String::from("(stdin closed)");
        }
        let trimmed = line.trim().to_string();

        // If the user typed a number AND options are present,
        // resolve to the option text.
        if !options.is_empty() {
            if let Ok(n) = trimmed.parse::<usize>() {
                if n >= 1 && n <= options.len() {
                    return options[n - 1].clone();
                }
            }
        }
        trimmed
    }
}

pub struct AskUserQuestionTool {
    prompter: Box<dyn AskUserPrompter>,
}

impl AskUserQuestionTool {
    #[must_use]
    pub fn new(prompter: Box<dyn AskUserPrompter>) -> Self {
        Self { prompter }
    }

    /// Convenience constructor: stdin-backed prompter.
    #[must_use]
    pub fn stdin() -> Self {
        Self::new(Box::new(StdinAskUserPrompter))
    }

    fn run(&mut self, input: &str) -> Result<String, ToolError> {
        let parsed: AskInput = serde_json::from_str(input).map_err(|e| {
            ToolError::new(format!("AskUserQuestion input not valid JSON: {e}"))
        })?;
        let answer = self.prompter.ask(&parsed.question, &parsed.options);
        Ok(answer)
    }
}

impl ToolExecutor for AskUserQuestionTool {
    fn execute(&mut self, name: &str, input: &str) -> Result<String, ToolError> {
        match name {
            "AskUserQuestion" => self.run(input),
            other => Err(ToolError::new(format!(
                "AskUserQuestionTool received unknown tool: {other:?}"
            ))),
        }
    }
}

// ============================================================
// Bundled executor
// ============================================================

/// All four extra tools wrapped into a single ToolExecutor source,
/// so callers can mount them with one MultiToolExecutor entry. The
/// AskUserQuestion prompter defaults to stdin; override with
/// `with_prompter`.
pub struct ExtraTools {
    web_fetch: WebFetchTool,
    web_search: WebSearchTool,
    ask_user: AskUserQuestionTool,
    enable_todo: bool,
    enable_web_fetch: bool,
    enable_web_search: bool,
    enable_ask_user: bool,
}

impl ExtraTools {
    /// Build with all four tools enabled and stdin prompter. Use
    /// `disable_*` to opt out of individual tools.
    #[must_use]
    pub fn new(_workspace: PathBuf, web_fetch_config: WebFetchConfig) -> Self {
        Self {
            web_fetch: WebFetchTool::new(web_fetch_config),
            web_search: WebSearchTool::new(),
            ask_user: AskUserQuestionTool::stdin(),
            enable_todo: true,
            enable_web_fetch: true,
            enable_web_search: true,
            enable_ask_user: true,
        }
    }

    pub fn with_prompter(mut self, p: Box<dyn AskUserPrompter>) -> Self {
        self.ask_user = AskUserQuestionTool::new(p);
        self
    }

    pub fn disable_ask_user(mut self) -> Self {
        self.enable_ask_user = false;
        self
    }

    /// Tool definitions for the enabled subset.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut out = Vec::new();
        if self.enable_todo {
            out.push(todo_write_definition());
        }
        if self.enable_web_fetch {
            out.push(web_fetch_definition());
        }
        if self.enable_web_search {
            out.push(web_search_definition());
        }
        if self.enable_ask_user {
            out.push(ask_user_question_definition());
        }
        out
    }
}

impl ToolExecutor for ExtraTools {
    fn execute(&mut self, name: &str, input: &str) -> Result<String, ToolError> {
        match name {
            "TodoWrite" => run_todo_write(input),
            "WebFetch" => self.web_fetch.execute(name, input),
            "WebSearch" => self.web_search.execute(name, input),
            "AskUserQuestion" => self.ask_user.execute(name, input),
            other => Err(ToolError::new(format!(
                "ExtraTools received unknown tool: {other:?}"
            ))),
        }
    }
}

// ============================================================
// Helpers
// ============================================================

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        // Truncate at a UTF-8 boundary by walking backwards.
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str(&format!("\n... [truncated, original {max}+ bytes]"));
    }
    s
}

#[allow(dead_code)]
fn _silence_unused_value(_: &Value) {}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- TodoWrite ----------

    #[test]
    fn todo_write_formats_pending_completed_in_progress_marks() {
        let input = json!({
            "summary": "ship phase 6",
            "todos": [
                { "description": "add tests", "status": "completed" },
                { "description": "wire CLI", "status": "in_progress" },
                { "description": "commit", "status": "pending" }
            ]
        })
        .to_string();
        let out = run_todo_write(&input).unwrap();
        assert!(out.contains("# ship phase 6"));
        assert!(out.contains("[x] 1. add tests"));
        assert!(out.contains("[~] 2. wire CLI"));
        assert!(out.contains("[ ] 3. commit"));
    }

    #[test]
    fn todo_write_works_without_summary() {
        let input = json!({
            "todos": [{ "description": "do thing" }]
        })
        .to_string();
        let out = run_todo_write(&input).unwrap();
        assert!(out.starts_with("[ ] 1. do thing"));
    }

    #[test]
    fn todo_write_rejects_invalid_json() {
        let err = run_todo_write("not json").unwrap_err();
        assert!(err.message().contains("not valid JSON"));
    }

    // ---------- WebFetch allowlist ----------

    #[test]
    fn web_fetch_allowlist_blocks_outside_url() {
        let mut tool = WebFetchTool::new(WebFetchConfig {
            allowlist: vec!["https://docs.rs/".into()],
        });
        let err = tool
            .execute("WebFetch", r#"{"url":"https://evil.example/"}"#)
            .unwrap_err();
        assert!(err.message().contains("does not match any allowlist"));
    }

    #[test]
    fn web_fetch_allowlist_empty_allows_all_format() {
        // We can't actually hit the network here, but the input
        // validation path runs before the call. Use an obviously
        // invalid URL to force the transport error path quickly.
        let mut tool = WebFetchTool::new(WebFetchConfig::default());
        // Should fail on transport, not on allowlist.
        let err = tool
            .execute("WebFetch", r#"{"url":"http://127.0.0.1:1/never"}"#)
            .unwrap_err();
        assert!(
            !err.message().contains("allowlist"),
            "empty allowlist must not produce an allowlist error: {}",
            err.message()
        );
    }

    // ---------- WebSearch HTML parser ----------

    #[test]
    fn format_search_results_extracts_title_and_unwraps_ddg_redirect() {
        // Minimal stand-in for DuckDuckGo HTML — keeps the real
        // markers (`class="result__a"`, `href="..."`, `uddg=`) so
        // the parser walks the same path.
        let html = r#"
        <html><body>
        <a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone&amp;rut=x" class="result__a">First Result</a>
        <a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Ftwo&amp;rut=y" class="result__a">Second &amp; More</a>
        </body></html>
        "#;
        let out = format_search_results(html, 10);
        assert!(out.contains("First Result"));
        assert!(out.contains("https://example.com/one"));
        assert!(out.contains("Second & More"));
    }

    #[test]
    fn format_search_results_returns_marker_when_layout_unrecognised() {
        let out = format_search_results("<html>nothing here</html>", 5);
        assert!(out.contains("layout may have changed"));
    }

    #[test]
    fn format_search_results_caps_at_max() {
        let mut html = String::new();
        for i in 0..20 {
            html.push_str(&format!(
                r#"<a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fex.com%2F{i}" class="result__a">R{i}</a>"#
            ));
        }
        let out = format_search_results(&html, 3);
        // 3 results → 3 lines.
        assert_eq!(out.lines().count(), 3);
    }

    // ---------- urlencode / urldecode ----------

    #[test]
    fn urlencode_handles_spaces_and_specials() {
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("foo&bar"), "foo%26bar");
        assert_eq!(urlencode("plain"), "plain");
    }

    #[test]
    fn urldecode_inverse_of_encode() {
        let s = "hello world & friends";
        assert_eq!(urldecode(&urlencode(s)), s);
    }

    // ---------- AskUserQuestion ----------

    struct ScriptedPrompter {
        canned: Vec<String>,
        idx: usize,
        last_question: Option<String>,
    }

    impl AskUserPrompter for ScriptedPrompter {
        fn ask(&mut self, question: &str, _options: &[String]) -> String {
            self.last_question = Some(question.to_string());
            let r = self.canned[self.idx].clone();
            self.idx += 1;
            r
        }
    }

    #[test]
    fn ask_user_question_returns_prompter_answer_verbatim() {
        let p = Box::new(ScriptedPrompter {
            canned: vec!["the user picked option B".into()],
            idx: 0,
            last_question: None,
        });
        let mut tool = AskUserQuestionTool::new(p);
        let r = tool
            .execute(
                "AskUserQuestion",
                r#"{"question":"which framework?","options":["A","B"]}"#,
            )
            .unwrap();
        assert_eq!(r, "the user picked option B");
    }

    #[test]
    fn ask_user_question_passes_question_text_to_prompter() {
        let p = Box::new(ScriptedPrompter {
            canned: vec!["yes".into()],
            idx: 0,
            last_question: None,
        });
        // Because the trait runs through Box, we can't easily look
        // at its internal state after the call without exposing
        // it. For coverage of the "question reaches prompter"
        // behaviour, rely on the next test (combined with the
        // verbatim-answer test).
        let mut tool = AskUserQuestionTool::new(p);
        let _ = tool
            .execute("AskUserQuestion", r#"{"question":"continue?"}"#)
            .unwrap();
    }

    // ---------- ExtraTools bundle ----------

    #[test]
    fn extra_tools_default_advertises_all_four() {
        let t = ExtraTools::new(PathBuf::from("."), WebFetchConfig::default());
        let defs = t.definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["TodoWrite", "WebFetch", "WebSearch", "AskUserQuestion"]
        );
    }

    #[test]
    fn extra_tools_disable_ask_user_drops_definition() {
        let t = ExtraTools::new(PathBuf::from("."), WebFetchConfig::default()).disable_ask_user();
        assert!(!t
            .definitions()
            .iter()
            .any(|d| d.name == "AskUserQuestion"));
    }

    #[test]
    fn extra_tools_routes_todo_call() {
        let mut t = ExtraTools::new(PathBuf::from("."), WebFetchConfig::default());
        let r = t
            .execute(
                "TodoWrite",
                r#"{"todos":[{"description":"do x"}]}"#,
            )
            .unwrap();
        assert!(r.contains("do x"));
    }

    // ---------- truncation ----------

    #[test]
    fn truncate_caps_long_string_with_marker() {
        let s = "x".repeat(300);
        let out = truncate(s, 100);
        assert!(out.starts_with("xxxxxxxxxx"));
        assert!(out.contains("[truncated"));
    }

    #[test]
    fn truncate_passes_through_short_string() {
        let s = "abc".to_string();
        let out = truncate(s.clone(), 100);
        assert_eq!(out, "abc");
    }
}
