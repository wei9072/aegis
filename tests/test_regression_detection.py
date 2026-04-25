"""
Boundary tests for cost-based regression detection.

Pin the contract change introduced when `_regressed` switched from
instance-count to total-value:

  - new file with all-zero signals → instance count grows, total
    cost does not → NOT regressed (legit splits no longer roll back)
  - same file count but a single signal value rose → cost grew →
    regressed (the case we always wanted to catch)

Without these locks, any future "let's count again" refactor would
silently revert the fix.
"""
from __future__ import annotations

from aegis.core.bindings import Signal
from aegis.runtime.pipeline import (
    _regressed,
    _regression_detail,
    _total_cost,
)


def _sig(name: str, value: float, *, file_path: str = "test.py", description: str = "") -> Signal:
    """Construct a Signal compatible with the Rust binding's
    constructor signature (name, value, description, file_path)."""
    return Signal(name=name, value=value, description=description, file_path=file_path)


def test_total_cost_sums_across_files():
    signals = {
        "a.py": [_sig("fan_out", 5)],
        "b.py": [_sig("fan_out", 3), _sig("max_chain_depth", 2)],
    }
    assert _total_cost(signals) == 10.0


def test_total_cost_zero_for_empty_signals():
    assert _total_cost({}) == 0.0
    assert _total_cost({"a.py": []}) == 0.0


def test_split_with_zero_cost_new_files_is_not_regression():
    """The whole point of the cost-based rewrite. god_module → user/
    billing/notification, where the new files have signal instances
    but value=0. Instance count grew (1 → 4 files) but total cost
    stays at 0. Must NOT report regression."""
    before = {"god_module.py": [_sig("fan_out", 0), _sig("max_chain_depth", 1)]}
    after = {
        "god_module.py": [_sig("fan_out", 0), _sig("max_chain_depth", 0)],
        "user.py": [_sig("fan_out", 0), _sig("max_chain_depth", 0)],
        "billing.py": [_sig("fan_out", 0), _sig("max_chain_depth", 0)],
        "notification.py": [_sig("fan_out", 0), _sig("max_chain_depth", 0)],
    }
    # cost: 1 → 0; instance count 2 → 8 (formerly would regress)
    assert not _regressed(before, after)


def test_value_growth_is_regression_even_with_same_file_count():
    """Inverse case: same number of files, but a metric got worse.
    Must report regression."""
    before = {"a.py": [_sig("fan_out", 5)]}
    after = {"a.py": [_sig("fan_out", 8)]}
    assert _regressed(before, after)


def test_regression_detail_lists_only_cost_growers():
    """detail dict carries kinds whose cost rose; kinds that shrank
    or stayed flat are omitted so the LLM sees ONLY what to address."""
    before = {
        "a.py": [_sig("fan_out", 10), _sig("max_chain_depth", 5)],
    }
    after = {
        "a.py": [_sig("fan_out", 4)],          # fan_out shrank
        "b.py": [_sig("max_chain_depth", 7)],  # chain depth grew
    }
    detail = _regression_detail(before, after)
    assert "max_chain_depth" in detail
    assert detail["max_chain_depth"] == 2.0  # 7 - 5
    # fan_out shrank (10 → 4); must NOT appear.
    assert "fan_out" not in detail


def test_regression_detail_empty_when_no_regression():
    before = {"a.py": [_sig("fan_out", 5)]}
    after = {"a.py": [_sig("fan_out", 3)]}
    assert _regression_detail(before, after) == {}


def test_equal_cost_is_not_regression():
    """Boundary: cost stays flat → not regressed (refactor was
    cost-neutral, e.g. moved logic without changing complexity)."""
    before = {"a.py": [_sig("max_chain_depth", 4)]}
    after = {
        "a.py": [_sig("max_chain_depth", 2)],
        "b.py": [_sig("max_chain_depth", 2)],
    }
    # split a deep chain into two helpers of equal depth — total 4 → 4
    assert not _regressed(before, after)
    assert _regression_detail(before, after) == {}
