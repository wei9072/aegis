"""
Higher-level graph traversal queries built on top of GraphService.
Extend here for blast-radius, fan-in, reachability, etc.
"""
from aegis.graph.service import GraphService


def blast_radius(service: GraphService, filepath: str) -> list[str]:
    """Placeholder: return files that would be affected if filepath changes."""
    raise NotImplementedError("blast_radius traversal — implement in Step 2")
