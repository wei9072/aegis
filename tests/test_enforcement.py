import pytest
from aegis.enforcement.validator import Ring0Enforcer


def test_ring0_valid_file_passes(tmp_path):
    f = tmp_path / "ok.py"
    f.write_text("def hello():\n    return 42\n")

    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))

    assert violations == []


def test_ring0_syntax_error_blocks(tmp_path):
    f = tmp_path / "bad.py"
    f.write_text("def err(\n")

    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))

    assert len(violations) == 1
    assert "[Ring 0]" in violations[0]


def test_ring0_high_fan_out_does_not_block(tmp_path):
    imports = "\n".join(f"import mod_{i}" for i in range(20))
    f = tmp_path / "heavy.py"
    f.write_text(imports + "\n")

    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))

    assert violations == [], "Ring 0 must NOT block on fan-out alone"


def test_ring0_deep_chain_does_not_block(tmp_path):
    f = tmp_path / "chain.py"
    f.write_text("a.b().c().d().e().f()\n")

    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))

    assert violations == [], "Ring 0 must NOT block on chain depth alone"


def test_ring0_circular_dep_blocks(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("from mod_a import Bar\n")

    enforcer = Ring0Enforcer()
    py_files = [str(tmp_path / "mod_a.py"), str(tmp_path / "mod_b.py")]
    violations = enforcer.check_project(py_files, root=str(tmp_path))

    assert len(violations) == 1
    assert "circular" in violations[0].lower()


def test_ring0_no_circular_dep_passes(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("x = 1\n")

    enforcer = Ring0Enforcer()
    py_files = [str(tmp_path / "mod_a.py"), str(tmp_path / "mod_b.py")]
    violations = enforcer.check_project(py_files, root=str(tmp_path))

    assert violations == []
