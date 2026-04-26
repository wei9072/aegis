"""
Thin wrapper around the Rust PyO3 extension at `aegis._core`.
All other layers import from here — never directly from `aegis._core` —
so swapping the Rust backend only requires changes in one place.

(Pre-V0.x the extension was importable as the top-level
`aegis_core_rs`; maturin mixed-mode now installs it as a submodule
of `aegis` instead, removing the manual `maturin develop` step from
the install sequence.)
"""
from aegis import _core as _rs

# Re-export Rust types
DependencyGraph = _rs.DependencyGraph
AstMetrics = _rs.AstMetrics
Signal = _rs.Signal
IrNode = _rs.IrNode
IncrementalUpdater = _rs.IncrementalUpdater

# Re-export Rust functions
ring0_status = _rs.ring0_status
get_imports = _rs.get_imports
analyze_file = _rs.analyze_file
check_syntax = _rs.check_syntax
extract_signals = _rs.extract_signals
extract_ts_imports = _rs.extract_ts_imports
build_ir = _rs.build_ir
supported_languages = _rs.supported_languages
supported_extensions = _rs.supported_extensions
