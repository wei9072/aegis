import pytest
from aegis.analysis.signals import SignalLayer


def test_signal_layer_extracts_fan_out(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\nfrom typing import List\n")

    layer = SignalLayer()
    signals = layer.extract(str(f))

    names = {s.name for s in signals}
    assert "fan_out" in names
    assert "max_chain_depth" in names


def test_signal_layer_fan_out_value(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\n")

    layer = SignalLayer()
    signals = layer.extract(str(f))

    fan_out = next(s for s in signals if s.name == "fan_out")
    assert fan_out.value == 2.0


def test_signal_layer_format_for_llm(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\n")

    layer = SignalLayer()
    signals = layer.extract(str(f))
    text = layer.format_for_llm(signals)

    assert "fan_out" in text
    assert "max_chain_depth" in text
    assert "signal" in text.lower()


def test_signal_layer_never_raises_on_valid_code(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("x = 1\n")

    layer = SignalLayer()
    signals = layer.extract(str(f))
    assert isinstance(signals, list)


def test_format_for_llm_empty_signals():
    layer = SignalLayer()
    text = layer.format_for_llm([])
    assert isinstance(text, str)
