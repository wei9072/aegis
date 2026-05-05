//! `aegis-core` — V2 analysis primitives.
//!
//! Two infrastructure layers + finding generators:
//!
//! - **Layer 1 (parse)**: `ast::parse(path, source) -> ParsedFile`
//!   produces a tree-sitter tree shared by all downstream consumers.
//!   Always returns Some when an adapter exists, even on broken
//!   syntax — judgment about syntactic validity is a finding, not a
//!   parse-layer short-circuit.
//!
//! - **Layer 2 (workspace)**: `workspace::WorkspaceIndex` aggregates
//!   per-file `FileSummary` records (imports, public symbols, signal
//!   values) into a reverse index that supports fan_in / cycle /
//!   role / z-score queries. mtime-cached via `aegis-index`.
//!
//! - **Findings (`findings::gather_findings*`)**: the V2 wire format.
//!   Each finding is a fact (Syntax / Signal / Security / Workspace
//!   kind) with file + range + context. No decision, no severity.
//!   The consuming agent decides what to act on.
//!
//! V1 ValidateVerdict / decision / reasons / severity all removed.

pub mod ast;
pub mod ir;
pub mod graph;
pub mod signals;
pub mod incremental;
pub mod enforcement;
pub mod findings;
pub mod security;
pub mod workspace;
pub mod signal_layer_pyapi;

pub use findings::{
    gather_findings, gather_findings_with_workspace, Finding, FindingKind, Range,
    FINDINGS_SCHEMA_VERSION,
};
