"""
Aegis MCP server — minimum viable implementation per docs/integrations/mcp_design.md.

V0.x scope: just the `validate_change` tool. `validate_diff` and
`get_signals` deferred per the design doc — add only when external
demand justifies them.

Run as a server (typical Cursor / Claude Code setup):
    python -m aegis_mcp

Or via the entry point:
    aegis-mcp
"""
from aegis_mcp.server import main

__all__ = ["main"]
