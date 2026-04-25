"""
Tests for DecisionTrace + gate emit instrumentation.

These tests assert on event sequences, not on output strings — that is
the whole point of the trace: the eval harness verifies *which decisions*
the system made, not *what it returned*.
"""
import pytest

from aegis.agents.llm_adapter import LLMGateway, Ring0Validator, SignalContextBuilder
from aegis.analysis.signals import SignalLayer
from aegis.enforcement.validator import Ring0Enforcer
from aegis.runtime.trace import BLOCK, OBSERVE, PASS, DecisionEvent, DecisionTrace


# ---------- DecisionTrace primitives ----------

def test_trace_starts_empty():
    trace = DecisionTrace()
    assert trace.events == []
    assert not trace.has_block()


def test_trace_emit_appends_in_order():
    trace = DecisionTrace()
    trace.emit("ring0", PASS, reason="syntax_valid")
    trace.emit("ring0_5", OBSERVE, reason="fan_out", signals={"fan_out": 3.0})

    assert len(trace.events) == 2
    assert trace.events[0].layer == "ring0"
    assert trace.events[1].layer == "ring0_5"
    assert trace.events[1].signals["fan_out"] == 3.0


def test_trace_query_helpers():
    trace = DecisionTrace()
    trace.emit("ring0", PASS, reason="syntax_valid")
    trace.emit("ring0", BLOCK, reason="circular_dependency")
    trace.emit("ring0_5", OBSERVE, reason="fan_out")

    assert len(trace.by_layer("ring0")) == 2
    assert len(trace.by_decision(BLOCK)) == 1
    assert trace.has_block()
    assert trace.reasons() == ["syntax_valid", "circular_dependency", "fan_out"]


def test_trace_to_list_serializable():
    trace = DecisionTrace()
    trace.emit("ring0", BLOCK, reason="syntax_invalid", metadata={"path": "/tmp/x.py"})

    events = trace.to_list()
    assert len(events) == 1
    assert events[0]["layer"] == "ring0"
    assert events[0]["decision"] == BLOCK
    assert events[0]["metadata"]["path"] == "/tmp/x.py"


# ---------- Ring0Enforcer emits ----------

def test_ring0_enforcer_check_file_emits_pass(tmp_path):
    f = tmp_path / "ok.py"
    f.write_text("def hello():\n    return 42\n")

    trace = DecisionTrace()
    Ring0Enforcer().check_file(str(f), trace=trace)

    events = trace.by_layer("ring0")
    assert len(events) == 1
    assert events[0].decision == PASS
    assert events[0].reason == "syntax_valid"


def test_ring0_enforcer_check_file_emits_block(tmp_path):
    f = tmp_path / "bad.py"
    f.write_text("def err(\n")

    trace = DecisionTrace()
    Ring0Enforcer().check_file(str(f), trace=trace)

    events = trace.by_layer("ring0")
    assert len(events) == 1
    assert events[0].decision == BLOCK
    assert events[0].reason == "syntax_invalid"
    assert events[0].metadata["violations"]


def test_ring0_enforcer_check_project_circular_emits_block(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("from mod_a import Bar\n")
    py_files = [str(tmp_path / "mod_a.py"), str(tmp_path / "mod_b.py")]

    trace = DecisionTrace()
    Ring0Enforcer().check_project(py_files, root=str(tmp_path), trace=trace)

    block_events = [e for e in trace.by_layer("ring0") if e.decision == BLOCK]
    assert len(block_events) == 1
    assert block_events[0].reason == "circular_dependency"


def test_ring0_enforcer_check_project_no_cycle_emits_pass(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("x = 1\n")
    py_files = [str(tmp_path / "mod_a.py"), str(tmp_path / "mod_b.py")]

    trace = DecisionTrace()
    Ring0Enforcer().check_project(py_files, root=str(tmp_path), trace=trace)

    events = trace.by_layer("ring0")
    assert len(events) == 1
    assert events[0].decision == PASS
    assert events[0].reason in {"no_cycle", "circular_dep_no_internal_edges"}


def test_ring0_enforcer_omits_trace_when_none(tmp_path):
    """Backwards compat: callers that pass no trace still get violations only."""
    f = tmp_path / "bad.py"
    f.write_text("def err(\n")

    violations = Ring0Enforcer().check_file(str(f))
    assert len(violations) == 1


# ---------- SignalLayer emits ----------

def test_signal_layer_emits_observe_per_signal(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\nfrom typing import List\n")

    trace = DecisionTrace()
    signals = SignalLayer().extract(str(f), trace=trace)

    observe_events = trace.by_layer("ring0_5")
    assert len(observe_events) == len(signals)
    assert all(e.decision == OBSERVE for e in observe_events)

    fan_out_events = [e for e in observe_events if e.reason == "fan_out"]
    assert len(fan_out_events) == 1
    assert fan_out_events[0].signals["fan_out"] >= 2.0


def test_signal_layer_no_trace_works(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\n")
    signals = SignalLayer().extract(str(f))
    assert isinstance(signals, list)


# ---------- LLMGateway end-to-end trace ----------

class _FakeProvider:
    def __init__(self, responses):
        self._responses = iter(responses)

    def generate(self, prompt: str) -> str:
        return next(self._responses)


def test_gateway_records_trace_on_success():
    gw = LLMGateway(llm_provider=_FakeProvider(["x = 1"]))
    gw.generate_and_validate("write x = 1")

    trace = gw.last_trace
    assert trace is not None

    layers = [e.layer for e in trace.events]
    decisions = [e.decision for e in trace.events]
    reasons = [e.reason for e in trace.events]

    # Expected sequence: gateway started → ring0 pass → gateway accepted
    assert layers[0] == "gateway"
    assert reasons[0] == "request_started"
    assert "ring0" in layers
    assert PASS in decisions
    assert reasons[-1] == "response_accepted"
    assert not trace.has_block()


def test_gateway_records_trace_on_retry_then_success():
    gw = LLMGateway(llm_provider=_FakeProvider([
        "```python\ndef err(\n```",
        "```python\ndef ok():\n    pass\n```",
    ]))
    gw.generate_and_validate("write a function")

    trace = gw.last_trace
    assert trace is not None

    ring0 = trace.by_layer("ring0")
    # First Ring 0 emit must block, second must pass.
    assert ring0[0].decision == BLOCK
    assert ring0[0].reason == "syntax_invalid"
    assert ring0[-1].decision == PASS

    retries = [e for e in trace.by_layer("gateway") if e.reason == "retry"]
    assert len(retries) == 1


def test_gateway_records_trace_on_max_retries():
    gw = LLMGateway(llm_provider=_FakeProvider(["```python\ndef err(\n```"] * 5))
    with pytest.raises(RuntimeError):
        gw.generate_and_validate("bad", max_retries=3)

    trace = gw.last_trace
    assert trace is not None
    assert trace.has_block()
    final = trace.events[-1]
    assert final.layer == "gateway"
    assert final.reason == "max_retries_exhausted"
    assert final.metadata["attempts"] == 3


def test_gateway_emits_ring0_5_signals_on_accepted_response():
    code = "```python\nimport os\nimport sys\nimport json\nx = 1\n```"
    gw = LLMGateway(llm_provider=_FakeProvider([code]))
    gw.generate_and_validate("write code with imports")

    trace = gw.last_trace
    assert trace is not None
    ring0_5 = trace.by_layer("ring0_5")
    assert ring0_5, "Ring 0.5 must emit observe events when code is accepted"
    fan_out_event = next((e for e in ring0_5 if e.reason == "fan_out"), None)
    assert fan_out_event is not None
    assert fan_out_event.signals["fan_out"] >= 3.0
