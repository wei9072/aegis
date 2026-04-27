//! `LLMPlanner` — port of `aegis/agents/planner.py`. Pure Rust.
//!
//! Takes an `&dyn LLMProvider` + a `PlanContext` (defined in
//! `aegis-runtime` so the loop and the planner share one source of
//! truth), builds the Markdown-flavoured prompt the V0.x Python
//! prompt template produced (kept verbatim so existing tuning +
//! corpora keep working), calls `provider.generate(prompt)`,
//! extracts the JSON plan block, and parses into an
//! `aegis_ir::PatchPlan`. Bounded retries on parse failure.

use aegis_ir::{plan_from_json, PatchPlan};
use aegis_runtime::{PlanContext, Planner, PlannerError as RuntimePlannerError};
use serde_json::Value;
use thiserror::Error;

use crate::error::ProviderError;
use crate::LLMProvider;

#[derive(Debug, Error)]
pub enum PlannerError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("plan response had no JSON block")]
    NoJson,
    #[error("plan JSON top-level was not an object")]
    NotAnObject,
    #[error("plan JSON failed to parse: {0}")]
    BadJson(String),
    #[error("plan failed to deserialize: {0}")]
    BadShape(String),
    #[error("planner exhausted {attempts} attempts; last error: {last}")]
    OutOfRetries { attempts: u32, last: String },
}

const PLAN_SCHEMA_HINT: &str = r#"{
  "goal": "<your understanding of the user's task>",
  "strategy": "<one-paragraph approach>",
  "target_files": ["relative/path.py", "..."],
  "patches": [
    {
      "id": "p1",
      "kind": "modify",          // "create" | "modify" | "delete"
      "path": "relative/path.py",
      "rationale": "why this patch",
      "content": "<full file body, CREATE only>",
      "edits": [
        {
          "old_string": "<exact text to find, must be unique in the file>",
          "new_string": "<replacement>",
          "context_before": "<>=1 line of surrounding text above old_string>",
          "context_after":  "<>=1 line of surrounding text below old_string>"
        }
      ]
    }
  ],
  "done": false   // set true when you believe the task is complete
}
"#;

/// Bounded-retry planner. `max_retries` defaults to 2 (matches V0.x).
pub struct LLMPlanner<'a> {
    provider: &'a dyn LLMProvider,
    pub max_retries: u32,
}

impl<'a> LLMPlanner<'a> {
    pub fn new(provider: &'a dyn LLMProvider) -> Self {
        Self {
            provider,
            max_retries: 2,
        }
    }

    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Generate one PatchPlan for `ctx`. Bounded retries on JSON-parse
    /// failure (mirrors V0.x: retry prompts include the error so the
    /// LLM can self-correct).
    pub fn plan(&self, ctx: &PlanContext) -> Result<PatchPlan, PlannerError> {
        let prompt = self.format_prompt(ctx);
        let mut last_error: Option<String> = None;
        for attempt in 0..=self.max_retries {
            let prompt_to_send = if attempt == 0 {
                prompt.clone()
            } else {
                self.format_parse_retry(&prompt, last_error.as_deref())
            };
            let raw = self.provider.generate(&prompt_to_send)?;
            match self.try_parse(&raw) {
                Ok(plan) => return Ok(plan),
                Err(e) => last_error = Some(e.to_string()),
            }
        }
        Err(PlannerError::OutOfRetries {
            attempts: self.max_retries + 1,
            last: last_error.unwrap_or_default(),
        })
    }

    fn try_parse(&self, raw: &str) -> Result<PatchPlan, PlannerError> {
        let payload = extract_json_block(raw)?;
        let value: Value = serde_json::from_str(&payload)
            .map_err(|e| PlannerError::BadJson(e.to_string()))?;
        if !value.is_object() {
            return Err(PlannerError::NotAnObject);
        }
        plan_from_json(&value).map_err(|e| PlannerError::BadShape(e.to_string()))
    }

    fn format_prompt(&self, ctx: &PlanContext) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push("# Aegis Refactor Planner".to_string());
        parts.push(
            "You are an architecture-aware refactoring planner. Produce a \
             structured PatchPlan (JSON) that makes incremental progress toward \
             the user's task. Each MODIFY edit MUST include context_before / \
             context_after surrounding the old_string so the change can be \
             located unambiguously even if the code shifts."
                .to_string(),
        );
        parts.push("\n## Task".to_string());
        parts.push(ctx.task.clone());

        if let Some(scope) = &ctx.scope {
            if !scope.is_empty() {
                parts.push("\n## Scope (patches MUST stay inside these paths)".to_string());
                for s in scope {
                    parts.push(format!("- {s}"));
                }
            }
        }

        parts.push("\n## Project files".to_string());
        for f in ctx.py_files.iter().take(200) {
            parts.push(format!("- {f}"));
        }

        if ctx.has_cycle {
            parts.push("\n## Dependency cycle detected (Ring 0)".to_string());
            parts.push(
                "The project currently has a circular import; \
                 breaking it is high priority."
                    .to_string(),
            );
        }

        if !ctx.signals.is_empty() {
            parts.push("\n## Structural signals (Ring 0.5)".to_string());
            for (path, sigs) in &ctx.signals {
                if sigs.is_empty() {
                    continue;
                }
                parts.push(format!("\n### {path}"));
                for s in sigs {
                    parts.push(format!(
                        "- {} = {:.0}  ({})",
                        s.name, s.value, s.description
                    ));
                }
            }
        }

        if !ctx.file_snippets.is_empty() {
            parts.push("\n## File contents".to_string());
            for (path, body) in &ctx.file_snippets {
                parts.push(format!("\n### {path}"));
                parts.push("```python".to_string());
                parts.push(body.clone());
                parts.push("```".to_string());
            }
        }

        if let Some(prev) = &ctx.previous_plan {
            parts.push("\n## Previous attempt".to_string());
            parts.push(format!("Strategy: {}", prev.strategy));
            if !ctx.previous_errors.is_empty() {
                parts.push("Validator errors to fix:".to_string());
                for err in &ctx.previous_errors {
                    let mut loc = format!(
                        "patch={}",
                        err.patch_id.as_deref().unwrap_or("")
                    );
                    if let Some(idx) = err.edit_index {
                        loc.push_str(&format!(", edit={idx}"));
                    }
                    if err.matches > 0 {
                        loc.push_str(&format!(", matches={}", err.matches));
                    }
                    parts.push(format!(
                        "- [{}] {loc}: {}",
                        err.kind.as_str(),
                        err.message
                    ));
                }
            }
            if ctx.previous_regressed {
                parts.push(
                    "Previous plan APPLIED but was reverted because the \
                     post-apply total cost rose (regression). Specifically:"
                        .to_string(),
                );
                if !ctx.previous_regression_detail.is_empty() {
                    for (kind, delta) in &ctx.previous_regression_detail {
                        parts.push(format!(
                            "  - {kind} value increased by +{delta:.4}"
                        ));
                    }
                } else {
                    parts.push("  - (per-kind detail unavailable)".to_string());
                }
                parts.push(
                    "Try a different approach that keeps these costs \
                     non-increasing. Note: adding a new file with all-zero \
                     signals does NOT count as regression — only growth in \
                     actual signal values does."
                        .to_string(),
                );
            } else if let Some(res) = &ctx.previous_result {
                if !res.success {
                    parts.push("Execution failures:".to_string());
                    for r in &res.results {
                        let bad = r.error.is_some()
                            || !matches!(
                                r.status,
                                aegis_ir::PatchStatus::Applied
                                    | aegis_ir::PatchStatus::AlreadyApplied
                            );
                        if bad {
                            parts.push(format!(
                                "- patch={} status={} matches={} err={}",
                                r.patch_id,
                                r.status.as_str(),
                                r.matches,
                                r.error.as_deref().unwrap_or("")
                            ));
                        }
                    }
                }
            }
            parts.push(
                "Produce a revised plan. If matches>1, expand context_before / \
                 context_after until the anchor is unique. If previous edits \
                 were correct, set done=true and return an empty patches list."
                    .to_string(),
            );
        }

        parts.push("\n## Output".to_string());
        parts.push("Return ONLY a fenced JSON block matching this schema:".to_string());
        parts.push("```json".to_string());
        parts.push(PLAN_SCHEMA_HINT.trim_end().to_string());
        parts.push("```".to_string());
        parts.join("\n")
    }

    fn format_parse_retry(&self, original: &str, error: Option<&str>) -> String {
        let err = error.unwrap_or("(unknown error)");
        format!(
            "{original}\n\n\
             Previous response could not be parsed as a PatchPlan: {err}\n\
             Return ONLY the JSON block. No prose, no explanation outside the block."
        )
    }
}

/// Implement `aegis_runtime::Planner` so the native pipeline loop
/// can take any LLMPlanner as a trait object. The `&mut PlanContext`
/// parameter is read-only here (the loop sets previous_* fields
/// before calling), so there's no Rust-side mutation in this impl.
impl<'a> Planner for LLMPlanner<'a> {
    fn plan(&self, ctx: &mut PlanContext) -> Result<PatchPlan, RuntimePlannerError> {
        LLMPlanner::plan(self, ctx).map_err(|e| RuntimePlannerError::Failed(e.to_string()))
    }
}

fn extract_json_block(text: &str) -> Result<String, PlannerError> {
    if let Some(start) = text.find("```json") {
        let after = &text[start + "```json".len()..];
        let after = after.trim_start_matches(|c: char| c.is_whitespace());
        if let Some(end) = after.find("```") {
            let payload = after[..end].trim_end_matches(|c: char| c.is_whitespace());
            return Ok(payload.to_string());
        }
    }
    let start = text.find('{').ok_or(PlannerError::NoJson)?;
    let end = text.rfind('}').ok_or(PlannerError::NoJson)?;
    if end < start {
        return Err(PlannerError::NoJson);
    }
    Ok(text[start..=end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ProviderError;
    use std::sync::Mutex;

    struct StubProvider {
        responses: Mutex<Vec<String>>,
    }

    impl StubProvider {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(
                    responses.into_iter().map(String::from).collect(),
                ),
            }
        }
    }

    impl LLMProvider for StubProvider {
        fn generate(&self, _prompt: &str) -> Result<String, ProviderError> {
            let mut r = self.responses.lock().unwrap();
            if r.is_empty() {
                return Err(ProviderError::BadResponse {
                    url: "stub://".into(),
                    body: "no more stub responses".into(),
                });
            }
            Ok(r.remove(0))
        }
        fn name(&self) -> &str {
            "stub"
        }
    }

    fn good_plan_json() -> &'static str {
        r#"```json
{
  "goal": "rename original to renamed",
  "strategy": "single MODIFY",
  "target_files": ["a.py"],
  "patches": [
    {
      "id": "p1",
      "kind": "modify",
      "path": "a.py",
      "rationale": "the rename",
      "edits": [
        {
          "old_string": "original",
          "new_string": "renamed",
          "context_before": "header\n",
          "context_after": "\nfooter"
        }
      ]
    }
  ],
  "done": true
}
```"#
    }

    fn ctx_with_task(task: &str) -> PlanContext {
        PlanContext {
            task: task.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn happy_path_parses_first_response() {
        let p = StubProvider::new(vec![good_plan_json()]);
        let planner = LLMPlanner::new(&p);
        let ctx = ctx_with_task("rename");
        let plan = planner.plan(&ctx).expect("plan parses");
        assert_eq!(plan.goal, "rename original to renamed");
        assert_eq!(plan.patches.len(), 1);
    }

    #[test]
    fn extract_json_block_handles_fenced_form() {
        let raw = "preamble\n```json\n{\"a\":1}\n```\nepilogue";
        assert_eq!(extract_json_block(raw).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn extract_json_block_falls_back_to_braces() {
        let raw = "no fence but {\"a\": 2} is here";
        assert_eq!(extract_json_block(raw).unwrap(), "{\"a\": 2}");
    }

    #[test]
    fn extract_json_block_returns_no_json_when_absent() {
        let raw = "no json at all";
        assert!(matches!(extract_json_block(raw), Err(PlannerError::NoJson)));
    }

    #[test]
    fn plan_retries_on_parse_failure_and_recovers() {
        let p = StubProvider::new(vec!["garbage", "still no json", good_plan_json()]);
        let planner = LLMPlanner::new(&p);
        let ctx = ctx_with_task("rename");
        let plan = planner.plan(&ctx).expect("eventually parses");
        assert_eq!(plan.goal, "rename original to renamed");
    }

    #[test]
    fn plan_exhausts_retries_when_all_responses_bad() {
        let p = StubProvider::new(vec!["bad", "bad", "bad"]);
        let planner = LLMPlanner::new(&p);
        let ctx = ctx_with_task("rename");
        let err = planner.plan(&ctx).unwrap_err();
        assert!(matches!(err, PlannerError::OutOfRetries { .. }));
    }

    #[test]
    fn planner_trait_impl_works_through_dyn_dispatch() {
        let p = StubProvider::new(vec![good_plan_json()]);
        let planner = LLMPlanner::new(&p);
        let dyn_planner: &dyn Planner = &planner;
        let mut ctx = ctx_with_task("rename");
        let plan = dyn_planner.plan(&mut ctx).expect("trait dispatch works");
        assert_eq!(plan.goal, "rename original to renamed");
    }
}
