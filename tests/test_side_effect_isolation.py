"""
Structural guards: the LLM must not have a direct write channel.

These tests are deliberately written against the static tool surface
declarations (not against runtime behaviour) so that any future PR which
re-introduces a mutating callable into the LLM-facing tool list fails
in CI before it ever reaches a real model.
"""
import inspect

import aegis.agents.gemini as gemini_mod
from aegis.agents.gemini import LLM_TOOLS_READ_ONLY
from aegis.tools import file_system


# Names of callables in aegis.tools.file_system that mutate filesystem state.
# Updating this set is intentional and should require a deliberate review.
MUTATING_TOOL_NAMES = {"write_file"}


def test_llm_tool_surface_is_read_only_by_name():
    names = {t.__name__ for t in LLM_TOOLS_READ_ONLY}
    assert names == {"read_file", "list_directory"}, (
        f"LLM tool surface drifted: {names}. "
        "Writes must go through aegis.runtime.executor.Executor."
    )


def test_no_mutating_tool_exposed_to_llm():
    exposed = {t.__name__ for t in LLM_TOOLS_READ_ONLY}
    leaked = exposed & MUTATING_TOOL_NAMES
    assert not leaked, f"Mutating tool(s) leaked into LLM surface: {leaked}"


def test_gemini_module_does_not_import_write_file():
    """write_file must not even be importable through the gemini module —
    otherwise a future edit could quietly add it to the tool list."""
    assert not hasattr(gemini_mod, "write_file"), (
        "aegis.agents.gemini imports write_file. Remove the import; "
        "writes belong to Executor, not to the LLM tool surface."
    )


def test_internal_write_file_still_exists_for_executor_paths():
    """write_file is intentionally retained as an internal helper for
    Executor and test fixtures. This test pins that contract so deleting
    it later is a deliberate decision."""
    assert callable(file_system.write_file)
    doc = inspect.getdoc(file_system.write_file) or ""
    assert "DO NOT expose" in doc or "Executor" in doc, (
        "write_file's docstring no longer warns against LLM exposure. "
        "Restore the policy comment."
    )
