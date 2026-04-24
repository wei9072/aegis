from aegis.core.bindings import check_syntax, get_imports, DependencyGraph
from aegis.ir.normalizer import build_module_map


class Ring0Enforcer:
    """Ring 0: deterministic, binary checks that block on violation."""

    def check_file(self, filepath: str) -> list[str]:
        return check_syntax(filepath)

    def check_project(self, py_files: list[str], root: str) -> list[str]:
        if len(py_files) < 2:
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
            return []
        dg = DependencyGraph()
        dg.build_from_edges(edges)
        if dg.check_circular_dependency():
            return [
                "[Ring 0] Circular dependency detected. "
                "Modules form a cycle (A→B→A). "
                "Extract a shared interface to break the loop."
            ]
        return []
