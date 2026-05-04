//! `aegis-core` — Ring 0 / Ring 0.5 analysis primitives.
//!
//! Pure-rlib crate. As of V1.10, all PyO3 surface has been removed
//! (the `aegis-pyshim` cdylib that consumed it was deleted in the
//! same release). Callers — `aegis-cli`, `aegis-mcp`, `aegis-runtime` —
//! use the `*_native` functions directly.

pub mod ast;
pub mod ir;
pub mod graph;
pub mod signals;
pub mod incremental;
pub mod attest;
pub mod enforcement;
pub mod reasons;
pub mod security;
pub mod scan;
pub mod validate;
pub mod workspace;
pub mod signal_layer_pyapi;
