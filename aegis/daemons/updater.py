"""
Incremental update pipeline: file change → AST parse → IR update → Graph update → Signal refresh.
"""
from aegis.ir.models import IRBuilder
from aegis.graph.service import GraphService


class IncrementalUpdater:
    def __init__(self, graph_service: GraphService) -> None:
        self._graph = graph_service
        self._ir = IRBuilder()

    def on_file_changed(self, filepath: str, root: str, all_files: list[str]) -> None:
        self._graph.build(all_files, root)
