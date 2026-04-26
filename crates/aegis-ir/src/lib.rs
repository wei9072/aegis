//! IR shared by Planner / Validator / Executor.
//!
//! `PatchPlan` is the contract. Planner produces it, Validator
//! verifies it, Executor applies it. Pure data + the pure
//! `apply_edit` / `apply_edits` engine — no I/O, no logic on top.
//!
//! Mirrors the Python modules `aegis.ir.patch` + `aegis.shared.edit_engine`
//! one-for-one. Rust is the ground-truth implementation; PyO3 wrappers
//! in `aegis-pyshim` re-export everything below.

pub mod edit_engine;
pub mod patch;

pub use edit_engine::{apply_edit, apply_edits, is_ok, EditResult};
pub use patch::{
    patch_from_json, patch_to_json, plan_from_json, plan_to_json, Edit, Patch, PatchKind,
    PatchPlan, PatchStatus,
};
