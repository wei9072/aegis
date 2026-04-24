"""
Python-side type aliases that mirror the Rust IR schema.
"""
from typing import TypeAlias
from aegis.core.bindings import Signal, AstMetrics, DependencyGraph, IrNode

SignalList: TypeAlias = list[Signal]
ViolationList: TypeAlias = list[str]   # violations are plain strings
IrNodeList: TypeAlias = list[IrNode]
