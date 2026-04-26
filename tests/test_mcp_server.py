"""
Unit tests for the aegis_mcp.server.validate_change tool.

These tests don't spin up the MCP transport — they call
validate_change as a regular Python function (which it remains
even after @mcp.tool() registration, since FastMCP's decorator is
non-wrapping). This keeps the tests fast (no stdio plumbing) while
still pinning the contract pinned in docs/integrations/mcp_design.md.

Critical contract checks:
  - PASS / WARN / BLOCK aggregation priority
  - reasons[] contains structured (not natural-language) detail
  - signals_after always present; signals_before only when old_content given
  - regression_detail only when post-cost > pre-cost
"""
from __future__ import annotations

import pytest

# Skip the entire module when MCP isn't installed — keeps the main
# test suite green for users who don't opt into the mcp extra.
pytest.importorskip("mcp", reason="install with `pip install -e .[mcp]` to test")

from aegis_mcp.server import validate_change  # noqa: E402


# ---------- BLOCK paths ----------

def test_syntax_error_blocks():
    r = validate_change(path="bad.py", new_content="def add(a, b returns nothing")
    assert r["decision"] == "BLOCK"
    assert any(reason["layer"] == "ring0" for reason in r["reasons"])


def test_clean_code_passes():
    r = validate_change(path="good.py", new_content="def add(a, b):\n    return a + b\n")
    assert r["decision"] == "PASS"
    assert r["reasons"] == []
    assert "signals_after" in r
    assert "signals_before" not in r  # not provided


# ---------- regression detection (cost-aware) ----------

def test_regression_blocks_when_old_content_given():
    """15-import service.py should regress against an empty file."""
    old = "def x():\n    pass\n"
    new = "\n".join(f"import {m}" for m in [
        "os", "sys", "json", "re", "math", "time", "random", "hashlib",
        "base64", "datetime", "collections", "itertools", "functools",
        "pathlib", "typing",
    ]) + "\n\ndef x():\n    pass\n"
    r = validate_change(path="svc.py", new_content=new, old_content=old)

    # Decision is BLOCK due to cost_increased + (probably) high_fan_out.
    assert r["decision"] == "BLOCK"
    assert "signals_before" in r
    assert "signals_after" in r
    assert r["signals_after"]["fan_out"] > r["signals_before"]["fan_out"]
    # regression_detail surfaces the per-kind growers.
    assert "regression_detail" in r
    assert r["regression_detail"]["fan_out"] > 0


def test_no_regression_when_costs_equal():
    """Identical content → no cost change → no regression_detail."""
    content = "def x():\n    return 1\n"
    r = validate_change(path="svc.py", new_content=content, old_content=content)
    assert "signals_before" in r
    assert "regression_detail" not in r


# ---------- contract: reasons are structured, not coaching ----------

def test_reasons_carry_structured_fields_not_natural_language():
    """The whole point of the gate vocabulary: reasons MUST be
    machine-parseable {layer, decision, reason, detail} dicts. If a
    future PR replaces these with natural-language strings ('try
    reducing fan_out by removing imports'), Aegis has crossed from
    constraint system to coach. This test pins the contract."""
    # Distinct modules — fan_out counts unique imports, not lines.
    new = "\n".join(f"import {m}" for m in [
        "os", "sys", "json", "re", "math", "time", "random", "hashlib",
        "base64", "datetime", "collections", "itertools", "functools",
        "pathlib", "typing", "abc", "argparse", "asyncio", "bisect",
        "calendar", "csv", "ctypes", "decimal",
    ]) + "\ndef x(): pass\n"
    r = validate_change(path="svc.py", new_content=new)
    assert r["reasons"], "expected at least one reason for high-fan_out file"
    for reason in r["reasons"]:
        # Required structural fields:
        assert "layer" in reason
        assert "decision" in reason
        assert "reason" in reason
        # Forbidden field names that would indicate coaching:
        forbidden_substrings = ("hint", "suggestion", "fix", "advice", "should")
        for key in reason:
            for forbidden in forbidden_substrings:
                assert forbidden not in key.lower(), (
                    f"reason key {key!r} contains forbidden substring {forbidden!r} — "
                    "this looks like coaching, not constraint reporting. "
                    "See docs/gap3_control_plane.md#critical-principle."
                )
