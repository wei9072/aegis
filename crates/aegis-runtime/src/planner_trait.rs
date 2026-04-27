//! `Planner` + `ContextBuilder` traits for the native Rust pipeline.
//!
//! The Rust loop in `aegis-runtime::native_pipeline` (next module
//! over) takes `&dyn Planner` and `&dyn ContextBuilder` so callers
//! can plug in any implementation:
//!
//!   - `aegis-providers::LLMPlanner` — the production prompt-template
//!     LLMPlanner; implements `Planner`.
//!   - `WorkspaceContextBuilder` — provided here; calls
//!     `build_workspace_context` to produce a fresh `PlanContext`.
//!     Stateless; reused across iterations.
//!
//! Tests in `aegis-runtime` don't depend on a real LLM — they
//! provide a stub `Planner` that returns a canned `PatchPlan`.

use std::path::Path;

use aegis_ir::PatchPlan;
use thiserror::Error;

use crate::context::{build_workspace_context, ContextOptions, PlanContext};

#[derive(Debug, Error)]
pub enum PlannerError {
    #[error("planner failed: {0}")]
    Failed(String),
}

/// LLM-aware plan producer. Mirrors `aegis.agents.planner.LLMPlanner.plan`
/// shape: takes a mutable `PlanContext` (the loop sets `previous_*`
/// fields before each call) and returns a `PatchPlan` or an error.
///
/// Implementors handle their own retry / parse logic; the loop does
/// not retry on planner errors — a planner error terminates the loop
/// with `success=false` (matches V0.x).
pub trait Planner: Send + Sync {
    fn plan(&self, ctx: &mut PlanContext) -> Result<PatchPlan, PlannerError>;
}

/// Workspace state observer. Called once at loop start (to seed the
/// initial context) and again after each successful executor apply
/// (so the next iteration sees fresh signals + edges + snippets).
pub trait ContextBuilder: Send + Sync {
    fn build(
        &self,
        task: &str,
        root: &Path,
        scope: Option<&[String]>,
        include_snippets: bool,
    ) -> PlanContext;
}

/// Default `ContextBuilder` — calls `build_workspace_context` with
/// the V0.x defaults (max_snippets=30, max_listed_files=200).
pub struct WorkspaceContextBuilder;

impl ContextBuilder for WorkspaceContextBuilder {
    fn build(
        &self,
        task: &str,
        root: &Path,
        scope: Option<&[String]>,
        include_snippets: bool,
    ) -> PlanContext {
        let opts = ContextOptions {
            include_snippets,
            ..ContextOptions::default()
        };
        build_workspace_context(task, root, scope, &opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct StubPlanner {
        responses: Mutex<Vec<PatchPlan>>,
    }

    impl Planner for StubPlanner {
        fn plan(&self, _ctx: &mut PlanContext) -> Result<PatchPlan, PlannerError> {
            let mut r = self.responses.lock().unwrap();
            if r.is_empty() {
                return Err(PlannerError::Failed("no more responses".into()));
            }
            Ok(r.remove(0))
        }
    }

    #[test]
    fn workspace_context_builder_returns_populated_context() {
        let td = tempdir().unwrap();
        std::fs::write(td.path().join("a.py"), "import os\n").unwrap();
        let cb = WorkspaceContextBuilder;
        let ctx = cb.build("demo", td.path(), None, true);
        assert_eq!(ctx.task, "demo");
        assert!(ctx.py_files.iter().any(|f| f == "a.py"));
    }

    #[test]
    fn stub_planner_errors_when_exhausted() {
        let p = StubPlanner {
            responses: Mutex::new(Vec::new()),
        };
        let mut ctx = PlanContext::default();
        let err = p.plan(&mut ctx).unwrap_err();
        assert!(matches!(err, PlannerError::Failed(_)));
    }

}
