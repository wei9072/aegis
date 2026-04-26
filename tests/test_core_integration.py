from aegis import _core as aegis_core_rs
import tempfile
import os


def test_ring0_status():
    status = aegis_core_rs.ring0_status()
    assert status == "Ring 0 Rust Core Initialized"


def test_dependency_graph_cycle():
    dg = aegis_core_rs.DependencyGraph()
    dg.build_from_edges([("A.py", "B.py"), ("B.py", "C.py"), ("C.py", "A.py")])
    assert dg.check_circular_dependency() is True


def test_dependency_graph_no_cycle():
    dg = aegis_core_rs.DependencyGraph()
    dg.build_from_edges([("A.py", "B.py"), ("B.py", "C.py")])
    assert dg.check_circular_dependency() is False


def test_fan_out_graph_signal():
    dg = aegis_core_rs.DependencyGraph()
    dg.build_from_edges([
        ("main.py", "auth.py"), ("main.py", "db.py"), ("main.py", "utils.py"),
        ("app.py", "db.py"),
    ])
    violations = dg.check_max_fan_out(2)
    assert len(violations) == 1
    assert violations[0][0] == "main.py"


def test_check_syntax_valid():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(b"def hello():\n    return 42\n")
        f.flush()
    violations = aegis_core_rs.check_syntax(f.name)
    os.unlink(f.name)
    assert violations == []


def test_check_syntax_invalid():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(b"def err(\n")
        f.flush()
    violations = aegis_core_rs.check_syntax(f.name)
    os.unlink(f.name)
    assert len(violations) == 1
    assert "[Ring 0]" in violations[0]


def test_extract_signals_returns_expected_names():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(b"import os\nimport sys\n")
        f.flush()
    signals = aegis_core_rs.extract_signals(f.name)
    os.unlink(f.name)
    names = {s.name for s in signals}
    assert "fan_out" in names
    assert "max_chain_depth" in names


def test_ts_parser_integration():
    code = "import { auth } from './auth';\nimport React from 'react';\n"
    imports = aegis_core_rs.extract_ts_imports(code)
    assert "./auth" in imports
    assert "react" in imports
