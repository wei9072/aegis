"""
Aegis MCP server — exposes `validate_change` over MCP stdio transport.

This file is the minimum viable implementation of the contract pinned
in docs/integrations/mcp_design.md. Three deliberate design choices
worth knowing about:

1. **Only `validate_change` is exposed.** The design doc specifies
   `validate_diff` and `get_signals` as well. They are intentionally
   omitted from V0.x — add them when an external user files an issue
   needing them, not before. (See docs/post_launch_discipline.md.)

2. **No tools are exposed for retry / hint / explain.** The design
   doc enumerates these as "what is NOT exposed" with the framing
   reason: any tool that would let the agent treat the verdict as
   coaching turns Aegis into a goal-seeker. The tool returned here
   is pure observation — `{decision, reasons, signals_*}`. The agent
   decides what to do with it.

3. **The implementation is a thin adapter.** All decision logic
   stays inside `aegis.enforcement.validator`, `aegis.analysis.signals`,
   and `aegis.policy.engine`. This file just wraps them. If you find
   yourself adding new logic here, that's a sign the logic should
   move into `aegis/` first.
"""
from __future__ import annotations

import tempfile
from pathlib import Path
from typing import Any

from mcp.server.fastmcp import FastMCP

from aegis.analysis.signals import SignalLayer
from aegis.enforcement.validator import Ring0Enforcer
from aegis.policy.engine import PolicyEngine, BLOCK, WARN
from aegis.runtime.trace import DecisionTrace


mcp = FastMCP("aegis")

_enforcer = Ring0Enforcer()
_signals = SignalLayer()
_policy = PolicyEngine()


def _kind_value_totals(signals: list) -> dict[str, float]:
    """Sum signal values per kind — same shape used by the pipeline's
    cost-aware regression detector."""
    totals: dict[str, float] = {}
    for sig in signals:
        totals[sig.name] = totals.get(sig.name, 0.0) + float(sig.value)
    return totals


def _total_cost(signals: list) -> float:
    return sum(float(s.value) for s in signals)


@mcp.tool()
def validate_change(
    path: str,
    new_content: str,
    old_content: str | None = None,
) -> dict[str, Any]:
    """Run Aegis Ring 0 + structural-signal extraction + PolicyEngine
    on a proposed file write. Returns the decision verdict without
    applying the change.

    Args:
        path:        Path the agent intends to write (used as the
                     filename for syntax/structural analysis only —
                     no side effects to disk).
        new_content: Full file contents the agent intends to write.
        old_content: Optional. If provided, enables cost-aware
                     regression detection by comparing structural
                     signal totals before vs after.

    Returns:
        A dict with the verdict shape pinned in
        docs/integrations/mcp_design.md:

            {
              "decision": "PASS" | "WARN" | "BLOCK",
              "reasons": [{"layer", "decision", "reason", "detail"}, ...],
              "signals_after": {"fan_out": 5, ...},
              "signals_before": {...},      # only if old_content given
              "regression_detail": {...}    # only if regression detected
            }
    """
    reasons: list[dict[str, str]] = []
    suffix = Path(path).suffix or ".py"

    # Run Ring 0 on the new content. Use a temp file so the enforcer
    # works on a real path (existing API contract).
    with tempfile.NamedTemporaryFile(
        mode="w", suffix=suffix, delete=False, encoding="utf-8"
    ) as tmp:
        tmp.write(new_content)
        tmp_path = tmp.name

    try:
        for violation in _enforcer.check_file(tmp_path):
            reasons.append({
                "layer": "ring0",
                "decision": "block",
                "reason": "ring0_violation",
                "detail": str(violation),
            })

        # Extract structural signals on the new content.
        try:
            new_sigs = _signals.extract(tmp_path)
        except Exception as e:
            return {
                "decision": "BLOCK",
                "reasons": [{
                    "layer": "ring0_5",
                    "decision": "block",
                    "reason": "signal_extraction_failed",
                    "detail": f"{type(e).__name__}: {e}",
                }],
                "signals_after": {},
            }

        signals_after = _kind_value_totals(new_sigs)

        # Run PolicyEngine over the new signals via a synthetic trace
        # — PolicyEngine.evaluate consumes a DecisionTrace, mirroring
        # the Gateway path.
        trace = DecisionTrace()
        for sig in new_sigs:
            trace.emit(
                layer="ring0_5",
                decision="observe",
                reason=sig.name,
                signals={sig.name: float(sig.value)},
                metadata={"path": path},
            )
        verdict = _policy.evaluate(trace)
        for event in verdict.events:
            if event.decision in (BLOCK, WARN):
                reasons.append({
                    "layer": "policy",
                    "decision": event.decision,
                    "reason": event.reason,
                    "detail": (
                        f"signal {event.reason} crossed threshold; "
                        f"signals={event.signals}"
                    ),
                })

        result: dict[str, Any] = {
            "signals_after": signals_after,
            "reasons": reasons,
        }

        # Cost-aware regression check, only when old_content provided.
        if old_content is not None:
            with tempfile.NamedTemporaryFile(
                mode="w", suffix=suffix, delete=False, encoding="utf-8"
            ) as old_tmp:
                old_tmp.write(old_content)
                old_path = old_tmp.name
            try:
                try:
                    old_sigs = _signals.extract(old_path)
                except Exception:
                    old_sigs = []
                signals_before = _kind_value_totals(old_sigs)
                result["signals_before"] = signals_before
                cost_after = _total_cost(new_sigs)
                cost_before = _total_cost(old_sigs)
                if cost_after > cost_before:
                    growers = {
                        k: round(signals_after.get(k, 0.0) - signals_before.get(k, 0.0), 4)
                        for k in set(signals_after) | set(signals_before)
                        if signals_after.get(k, 0.0) > signals_before.get(k, 0.0)
                    }
                    result["regression_detail"] = growers
                    reasons.append({
                        "layer": "regression",
                        "decision": "block",
                        "reason": "cost_increased",
                        "detail": (
                            f"total cost {cost_before:g} → {cost_after:g}; "
                            f"growers: {growers}"
                        ),
                    })
            finally:
                Path(old_path).unlink(missing_ok=True)

        # Aggregate to a single decision. BLOCK dominates WARN
        # dominates PASS — same priority as the pipeline.
        if any(r["decision"] == "block" for r in reasons):
            result["decision"] = "BLOCK"
        elif any(r["decision"] == "warn" for r in reasons):
            result["decision"] = "WARN"
        else:
            result["decision"] = "PASS"

        return result
    finally:
        Path(tmp_path).unlink(missing_ok=True)


def main() -> None:
    """Entry point for `python -m aegis_mcp` / `aegis-mcp`."""
    mcp.run()


if __name__ == "__main__":
    main()
