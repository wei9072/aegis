"""
Unit tests for PolicyEngine — the deterministic rule layer that turns
Ring 0.5 observations into `policy:warn` / `policy:block` events.

These tests exercise the engine in isolation against a hand-built
DecisionTrace; the end-to-end signal → policy → delivery flow is
covered by the eval harness scenarios 04 / 05.
"""
from __future__ import annotations

from aegis.policy.engine import (
    DEFAULT_RULES,
    PolicyEngine,
    PolicyVerdict,
    SignalRule,
)
from aegis.runtime.trace import BLOCK, OBSERVE, WARN, DecisionTrace


def _emit_ring0_5(trace: DecisionTrace, name: str, value: float) -> None:
    trace.emit(
        layer="ring0_5",
        decision=OBSERVE,
        reason=name,
        signals={name: value},
    )


def test_empty_trace_yields_no_events():
    trace = DecisionTrace()
    verdict = PolicyEngine().evaluate(trace)
    assert verdict.events == []
    assert not verdict.has_block()
    assert verdict.warnings() == []
    assert trace.by_layer("policy") == []


def test_fan_out_below_threshold_does_not_fire():
    trace = DecisionTrace()
    _emit_ring0_5(trace, "fan_out", 5.0)
    verdict = PolicyEngine().evaluate(trace)
    assert verdict.events == []
    assert trace.by_layer("policy") == []


def test_fan_out_advisory_fires_at_threshold():
    trace = DecisionTrace()
    _emit_ring0_5(trace, "fan_out", 15.0)
    verdict = PolicyEngine().evaluate(trace)
    assert len(verdict.events) == 1
    ev = verdict.events[0]
    assert ev.layer == "policy"
    assert ev.decision == WARN
    assert ev.reason == "high_fan_out_advisory"
    assert ev.metadata["signal"] == "fan_out"
    assert ev.metadata["value"] == 15.0
    assert ev.metadata["threshold"] == 10.0
    assert verdict.warnings() == [ev]
    assert not verdict.has_block()


def test_fan_out_block_supersedes_warn():
    """A single fan_out=25 must fire only the strictest matching rule."""
    trace = DecisionTrace()
    _emit_ring0_5(trace, "fan_out", 25.0)
    verdict = PolicyEngine().evaluate(trace)
    assert len(verdict.events) == 1
    assert verdict.events[0].decision == BLOCK
    assert verdict.events[0].reason == "high_fan_out_block"
    assert verdict.has_block()
    # warn must NOT also fire for the same signal in the same pass.
    assert verdict.warnings() == []


def test_chain_depth_advisory_fires():
    trace = DecisionTrace()
    _emit_ring0_5(trace, "max_chain_depth", 5.0)
    verdict = PolicyEngine().evaluate(trace)
    assert len(verdict.events) == 1
    ev = verdict.events[0]
    assert ev.decision == WARN
    assert ev.reason == "demeter_violation_advisory"


def test_multiple_signals_each_fire_independently():
    trace = DecisionTrace()
    _emit_ring0_5(trace, "fan_out", 12.0)
    _emit_ring0_5(trace, "max_chain_depth", 6.0)
    verdict = PolicyEngine().evaluate(trace)
    reasons = sorted(e.reason for e in verdict.events)
    assert reasons == ["demeter_violation_advisory", "high_fan_out_advisory"]


def test_unknown_signals_are_ignored():
    """Adding new signals upstream must not crash the engine."""
    trace = DecisionTrace()
    _emit_ring0_5(trace, "totally_new_signal", 999.0)
    verdict = PolicyEngine().evaluate(trace)
    assert verdict.events == []


def test_engine_appends_to_caller_trace():
    """The engine emits into the same trace the caller passed in."""
    trace = DecisionTrace()
    _emit_ring0_5(trace, "fan_out", 15.0)
    PolicyEngine().evaluate(trace)
    policy_events = trace.by_layer("policy")
    assert len(policy_events) == 1
    assert policy_events[0].reason == "high_fan_out_advisory"


def test_custom_rules_are_data_driven():
    """Rules are data; swapping the rule set should not require code changes."""
    custom = (SignalRule("fan_out", 3.0, WARN, "tiny_threshold_test"),)
    trace = DecisionTrace()
    _emit_ring0_5(trace, "fan_out", 4.0)
    verdict = PolicyEngine(rules=custom).evaluate(trace)
    assert len(verdict.events) == 1
    assert verdict.events[0].reason == "tiny_threshold_test"


def test_default_rules_cover_phase_1_signals():
    """Regression guard: Phase 1 must keep these reasons stable."""
    reasons = {r.reason for r in DEFAULT_RULES}
    assert "high_fan_out_advisory" in reasons
    assert "high_fan_out_block" in reasons
    assert "demeter_violation_advisory" in reasons
