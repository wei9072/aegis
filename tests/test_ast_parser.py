from aegis import _core as aegis_core_rs
import pytest
import tempfile
import os

def test_ast_parser_syntax_error():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        # 寫入有語法錯誤的程式碼
        f.write(b"def my_func():\n    print('hello'\n")
        f.flush()
        
    metrics = aegis_core_rs.analyze_file(f.name)
    os.unlink(f.name)
    
    assert metrics.has_syntax_error is True

def test_ast_parser_fan_out():
    code = b"""
import os
import sys
from typing import List, Optional
import pydantic as pd
"""
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()
        
    metrics = aegis_core_rs.analyze_file(f.name)
    os.unlink(f.name)
    
    assert metrics.has_syntax_error is False
    # fan-out 應該包含 os, sys, typing, pydantic (約為 4 個)
    assert metrics.fan_out == 4

def test_ast_parser_chain_depth():
    code = b"""
def process():
    # depth = 3: a -> b() -> c() -> d()
    a.b().c().d()
"""
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()
        
    metrics = aegis_core_rs.analyze_file(f.name)
    os.unlink(f.name)
    
    assert metrics.has_syntax_error is False
    assert metrics.max_chain_depth == 3


def test_get_imports_basic():
    code = b"import os\nimport sys\nfrom typing import List\n"
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()

    result = aegis_core_rs.get_imports(f.name)
    os.unlink(f.name)

    assert "os" in result
    assert "sys" in result
    assert "typing" in result
    assert result == sorted(result)


def test_get_imports_deduplication():
    code = b"import os\nimport os\nfrom os import path\n"
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()

    result = aegis_core_rs.get_imports(f.name)
    os.unlink(f.name)

    assert result.count("os") == 1


def test_get_imports_no_imports():
    code = b"x = 1\nprint(x)\n"
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()

    result = aegis_core_rs.get_imports(f.name)
    os.unlink(f.name)

    assert result == []
