import pytest
from aegis.ir.models import SemanticNode, IRBuilder


def test_ir_builder_extracts_dependency_nodes(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nfrom typing import List\n")

    builder = IRBuilder()
    nodes = builder.build_from_file(str(f))

    dep_nodes = [n for n in nodes if n.type == "dependency"]
    assert len(dep_nodes) == 2
    names = {n.name for n in dep_nodes}
    assert "os" in names
    assert "typing" in names


def test_ir_builder_stores_file_path(tmp_path):
    f = tmp_path / "mod.py"
    f.write_text("import sys\n")

    builder = IRBuilder()
    nodes = builder.build_from_file(str(f))

    assert all(n.file_path == str(f) for n in nodes)


def test_ir_builder_empty_file(tmp_path):
    f = tmp_path / "empty.py"
    f.write_text("")

    builder = IRBuilder()
    nodes = builder.build_from_file(str(f))

    assert nodes == []


def test_semantic_node_to_dict():
    node = SemanticNode(type="dependency", file_path="/a.py", name="os", metadata={})
    d = node.to_dict()
    assert d["type"] == "dependency"
    assert d["name"] == "os"
    assert d["file_path"] == "/a.py"
