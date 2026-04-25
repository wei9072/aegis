from aegis.core.bindings import check_syntax, get_imports, DependencyGraph
from aegis.ir.normalizer import build_module_map
from aegis.runtime.trace import BLOCK, PASS, DecisionTrace


class Ring0Enforcer:
    """Ring 0: deterministic, binary checks that block on violation."""

    def check_file(
        self,
        filepath: str,
        trace: DecisionTrace | None = None,
    ) -> list[str]:
        violations = check_syntax(filepath)
        if trace is not None:
            if violations:
                trace.emit(
                    layer="ring0",
                    decision=BLOCK,
                    reason="syntax_invalid",
                    metadata={"path": filepath, "violations": list(violations)},
                )
            else:
                trace.emit(
                    layer="ring0",
                    decision=PASS,
                    reason="syntax_valid",
                    metadata={"path": filepath},
                )
        return violations

    def check_project(
        self,
        py_files: list[str],
        root: str,
        trace: DecisionTrace | None = None,
    ) -> list[str]:
        if len(py_files) < 2:
            if trace is not None:
                trace.emit(
                    layer="ring0",
                    decision=PASS,
                    reason="circular_dep_skipped_too_few_files",
                    metadata={"file_count": len(py_files)},
                )
            return []

        module_map = build_module_map(root, py_files)
        edges: list[tuple[str, str]] = []
        for py_file in py_files:
            try:
                for imp in get_imports(py_file):
                    if imp in module_map:
                        edges.append((py_file, module_map[imp]))
            except Exception:
                pass

        if not edges:
            if trace is not None:
                trace.emit(
                    layer="ring0",
                    decision=PASS,
                    reason="circular_dep_no_internal_edges",
                    metadata={"file_count": len(py_files)},
                )
            return []

        dg = DependencyGraph()
        dg.build_from_edges(edges)
        if dg.check_circular_dependency():
            if trace is not None:
                trace.emit(
                    layer="ring0",
                    decision=BLOCK,
                    reason="circular_dependency",
                    metadata={"edge_count": len(edges)},
                )
            return [
                "[Ring 0] Circular dependency detected. "
                "Modules form a cycle (A→B→A). "
                "Extract a shared interface to break the loop."
            ]

        if trace is not None:
            trace.emit(
                layer="ring0",
                decision=PASS,
                reason="no_cycle",
                metadata={"edge_count": len(edges)},
            )
        return []
