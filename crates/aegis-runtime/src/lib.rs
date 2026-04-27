//! Pipeline runtime primitives — the parts that don't carry a
//! language assumption.
//!
//! V1.2 ships the file snapshot/rollback primitive as a standalone
//! crate. The PatchPlan / EditEngine / PlanValidator data model
//! stays Python in V1.x (intertwined with planner-prompt shapes;
//! full port is post-V2 IR-model work).
//!
//! The intent: tools that need "snapshot N files → mutate → maybe
//! roll back" semantics (aegis-mcp's `validate_change`, future
//! Rust-native pipeline) can call directly into Rust without
//! depending on the Python executor.

pub mod context;
pub mod executor;
pub mod loop_step;
pub mod metrics;
pub mod native_pipeline;
pub mod planner_trait;
pub mod sequence;
pub mod snapshot;
pub mod validator;

pub use context::{
    build_module_map, build_workspace_context, discover_source_files, scope_filter,
    ContextOptions, PlanContext, Signal,
};
pub use executor::{ExecutionResult, Executor, ExecutorError, PatchResult};
pub use loop_step::{
    step_decision, LoopState, StepDecision, TerminateReason, STATE_STALEMATE_THRESHOLD,
    THRASHING_THRESHOLD,
};
pub use metrics::{
    hash_plan, kind_counts, kind_value_totals, regressed, regression_detail, total_cost,
    truncate_summary,
};
pub use native_pipeline::{run_pipeline, PipelineOptions, PipelineResult};
pub use planner_trait::{ContextBuilder, Planner, PlannerError, WorkspaceContextBuilder};
pub use sequence::{is_plan_repeat_stalemate, is_state_stalemate, is_thrashing};
pub use snapshot::{Snapshot, SnapshotError};
pub use validator::{ErrorKind, PlanValidator, ValidationError};
