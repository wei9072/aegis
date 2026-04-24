from aegis.core.bindings import DependencyGraph, get_imports
from aegis.ir.normalizer import build_module_map


class GraphService:
    """Builds and queries an import-based dependency graph for a set of files."""

    def __init__(self) -> None:
        self._graph = DependencyGraph()

    def build(self, py_files: list[str], root: str) -> None:
        module_map = build_module_map(root, py_files)
        edges: list[tuple[str, str]] = []
        for py_file in py_files:
            try:
                for imp in get_imports(py_file):
                    if imp in module_map:
                        edges.append((py_file, module_map[imp]))
            except Exception:
                pass
        if edges:
            self._graph.build_from_edges(edges)

    def has_cycle(self) -> bool:
        return self._graph.check_circular_dependency()

    def fan_out_violations(self, limit: int) -> list[tuple[str, int]]:
        return self._graph.check_max_fan_out(limit)
