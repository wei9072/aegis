import pytest
from click.testing import CliRunner
from aegis.cli import cli


def test_cli_help():
    runner = CliRunner()
    result = runner.invoke(cli, ['--help'])
    assert result.exit_code == 0
    assert 'Aegis Architecture Linter & Generator' in result.output


def test_cli_check_command_help():
    runner = CliRunner()
    result = runner.invoke(cli, ['check', '--help'])
    assert result.exit_code == 0
    assert 'Ring 0' in result.output


def test_cli_generate_command_help():
    runner = CliRunner()
    result = runner.invoke(cli, ['generate', '--help'])
    assert result.exit_code == 0
    assert 'Generate architecture-compliant code' in result.output


def test_cli_chat_command_help():
    runner = CliRunner()
    result = runner.invoke(cli, ['chat', '--help'])
    assert result.exit_code == 0
    assert 'Start an interactive chat session' in result.output


def test_cli_check_detects_circular_dependency(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("from mod_a import Bar\n")

    runner = CliRunner()
    result = runner.invoke(cli, ['check', str(tmp_path)])

    assert result.exit_code == 1
    assert "Circular" in result.output
    assert "Aegis check failed." in result.output


def test_cli_check_no_false_positive_on_clean_project(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("x = 1\n")

    runner = CliRunner()
    result = runner.invoke(cli, ['check', str(tmp_path)])

    assert result.exit_code == 0
    assert "Circular" not in result.output


def test_cli_check_single_file_skips_graph(tmp_path):
    (tmp_path / "solo.py").write_text("import os\n")

    runner = CliRunner()
    result = runner.invoke(cli, ['check', str(tmp_path)])

    assert result.exit_code == 0
    assert "Circular" not in result.output


def test_cli_check_signals_flag(tmp_path):
    (tmp_path / "app.py").write_text("import os\nimport sys\n")

    runner = CliRunner()
    result = runner.invoke(cli, ['check', str(tmp_path), '--signals'])

    assert result.exit_code == 0
    assert "fan_out" in result.output


def test_cli_check_high_fan_out_does_not_fail(tmp_path):
    imports = "\n".join(f"import mod_{i}" for i in range(20))
    (tmp_path / "heavy.py").write_text(imports + "\n")

    runner = CliRunner()
    result = runner.invoke(cli, ['check', str(tmp_path)])

    assert result.exit_code == 0, "High fan-out must NOT fail Ring 0 check"
    assert "Aegis check passed." in result.output
