"""
Thin wrapper around the aegis_core_rs PyO3 extension.
All other layers import from here — never directly from aegis_core_rs —
so swapping the Rust backend only requires changes in one place.
"""
import aegis_core_rs as _rs

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
