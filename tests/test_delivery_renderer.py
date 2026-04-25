"""
Unit tests for DeliveryRenderer — the formatting layer that keeps the
human-visible channel and the LLM-bound channel separate.

The renderer must:
  - leave both channels equal when there are no warnings (no surfaced flag)
  - emit `delivery:observe warning_surfaced` exactly once when banners exist
  - place the banner BEFORE the code in the human view
  - keep the LLM-bound channel identical to raw code (no banner leakage)
"""
from __future__ import annotations

from aegis.delivery.renderer import DeliveryRenderer
from aegis.policy.engine import PolicyEngine
from aegis.runtime.trace import OBSERVE, DecisionTrace


_CODE = "x = 1\n"


def _build_verdict_with_warning():
    trace = DecisionTrace()
    trace.emit(
        layer="ring0_5",
        decision=OBSERVE,
        reason="fan_out",
        signals={"fan_out": 15.0},
    )
    return trace, PolicyEngine().evaluate(trace)


def test_empty_verdict_leaves_code_untouched():
    trace = DecisionTrace()
    verdict = PolicyEngine().evaluate(trace)  # no signals → no events
    view = DeliveryRenderer().render(_CODE, verdict, trace=trace)
    assert view.human == _CODE
    assert view.llm == _CODE
    assert view.surfaced is False
    # No delivery event should have been emitted.
    assert trace.by_layer("delivery") == []


def test_warning_appears_before_code_in_human_view():
    trace, verdict = _build_verdict_with_warning()
    view = DeliveryRenderer().render(_CODE, verdict, trace=trace)
    assert view.surfaced is True
    banner_index = view.human.index("⚠️")
    code_index = view.human.index("x = 1")
    assert banner_index < code_index, "banner must precede code in human view"


def test_llm_view_never_contains_banner():
    """Delivery isolation: warnings must not enter LLM context."""
    trace, verdict = _build_verdict_with_warning()
    view = DeliveryRenderer().render(_CODE, verdict, trace=trace)
    assert "⚠️" not in view.llm
    assert "Warning" not in view.llm
    assert view.llm == _CODE


def test_renderer_emits_warning_surfaced_event():
    trace, verdict = _build_verdict_with_warning()
    DeliveryRenderer().render(_CODE, verdict, trace=trace)
    delivery_events = trace.by_layer("delivery")
    assert len(delivery_events) == 1
    ev = delivery_events[0]
    assert ev.decision == OBSERVE
    assert ev.reason == "warning_surfaced"
    assert ev.metadata["before_code"] is True
    assert "high_fan_out_advisory" in ev.metadata["warnings"]


def test_render_without_trace_still_returns_view():
    """`trace=None` must be accepted (renderer is usable in pure-format mode)."""
    _, verdict = _build_verdict_with_warning()
    view = DeliveryRenderer().render(_CODE, verdict, trace=None)
    assert view.surfaced is True
    assert "x = 1" in view.human


def test_unknown_reason_falls_back_to_raw_code():
    """A new policy reason must not produce an empty banner."""
    from aegis.policy.engine import PolicyVerdict
    from aegis.runtime.trace import DecisionEvent, WARN

    fake_event = DecisionEvent(
        layer="policy",
        decision=WARN,
        reason="brand_new_reason",
        metadata={"signal": "x", "value": 1, "threshold": 0},
    )
    verdict = PolicyVerdict(events=[fake_event])
    trace = DecisionTrace()
    view = DeliveryRenderer().render(_CODE, verdict, trace=trace)
    assert "brand_new_reason" in view.human
