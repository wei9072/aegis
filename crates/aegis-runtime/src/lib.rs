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

pub mod snapshot;
pub mod sequence;

pub use snapshot::{Snapshot, SnapshotError};
pub use sequence::{is_state_stalemate, is_thrashing, is_plan_repeat_stalemate};
