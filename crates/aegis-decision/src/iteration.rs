//! Minimal IterationEvent mirror — only the fields that
//! `derive_pattern` reads.
//!
//! The full Python `IterationEvent` carries diagnostic fields the
//! deriver doesn't consult; those will be ported in V1.3 when the
//! pipeline loop moves into Rust. For now this is a slim shim so
//! the per-iteration deriver is fully Rust-native.

#[derive(Clone, Debug, Default)]
pub struct IterationEvent {
    pub iteration: u32,
    pub plan_id: String,
    pub plan_done: bool,
    pub plan_patches: u32,
    pub validation_passed: bool,
    pub applied: bool,
    pub rolled_back: bool,
    pub regressed: bool,
    pub silent_done_contradiction: bool,
    pub stalemate_detected: bool,
    pub thrashing_detected: bool,
}
