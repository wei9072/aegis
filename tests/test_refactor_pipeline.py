"""Integration tests for Validator, Executor, and Pipeline."""
import json
from pathlib import Path

import pytest

from aegis.ir.patch import Edit, Patch, PatchKind, PatchPlan, PatchStatus
from aegis.runtime.executor import Executor
from aegis.runtime.validator import PlanValidator


@pytest.fixture
def workspace(tmp_path: Path) -> Path:
    (tmp_path / "a.py").write_text("header\noriginal\nfooter\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("alpha\nbeta\ngamma\n", encoding="utf-8")
    return tmp_path


def _modify_patch(
    path: str,
    old: str,
    new: str,
    *,
    before: str,
    after: str,
    pid: str = "p1",
) -> Patch:
    return Patch(
        id=pid, kind=PatchKind.MODIFY, path=path, rationale="test",
        edits=[Edit(old_string=old, new_string=new,
                    context_before=before, context_after=after)],
    )


# ------------- Validator -------------

def test_validator_accepts_valid_plan(workspace: Path):
    v = PlanValidator(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "a.py", "original", "renamed",
            before="header\n", after="\nfooter",
        )],
        target_files=["a.py"],
    )
    assert v.validate(plan) == []


def test_validator_rejects_path_escape(workspace: Path):
    v = PlanValidator(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "../evil.py", "x", "y",
            before="ctx\n", after="\nctx",
        )],
    )
    errs = v.validate(plan)
    assert any(e.kind == "path" for e in errs)


def test_validator_rejects_dangerous_dir(workspace: Path):
    v = PlanValidator(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            ".git/config", "a", "b",
            before="ctx\n", after="\nctx",
        )],
    )
    errs = v.validate(plan)
    assert any(e.kind == "dangerous_path" for e in errs)


def test_validator_enforces_scope(workspace: Path):
    (workspace / "sub").mkdir()
    (workspace / "sub" / "c.py").write_text("z = 3\n", encoding="utf-8")
    v = PlanValidator(str(workspace), scope=["sub"])
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "a.py", "original", "renamed",
            before="header\n", after="\nfooter",
        )],
    )
    errs = v.validate(plan)
    assert any(e.kind == "scope" for e in errs)


def test_validator_cross_patch_simulation_catches_stale(workspace: Path):
    """Patch 2 depends on text that patch 1 just removed. Must fail in simulation."""
    v = PlanValidator(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s", patches=[
            Patch(
                id="p1", kind=PatchKind.MODIFY, path="a.py", rationale="r",
                edits=[Edit(
                    old_string="original", new_string="renamed",
                    context_before="header\n", context_after="\nfooter",
                )],
            ),
            Patch(
                id="p2", kind=PatchKind.MODIFY, path="a.py", rationale="r",
                edits=[Edit(
                    old_string="original", new_string="new_name",
                    context_before="header\n", context_after="\nfooter",
                )],
            ),
        ],
    )
    errs = v.validate(plan)
    assert any(e.kind == "simulate_not_found" and e.patch_id == "p2" for e in errs), errs


def test_validator_enforces_target_files_commitment(workspace: Path):
    """When plan declares target_files, every patch.path must be in the list."""
    v = PlanValidator(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "b.py", "beta", "BETA",
            before="alpha\n", after="\ngamma",
        )],
        target_files=["a.py"],  # planner promised only a.py, but patch hits b.py
    )
    errs = v.validate(plan)
    assert any(e.kind == "scope" and "target_files" in e.message for e in errs), errs


def test_validator_rejects_modify_without_context(workspace: Path):
    v = PlanValidator(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s", patches=[
            Patch(id="p1", kind=PatchKind.MODIFY, path="a.py", rationale="r",
                  edits=[Edit(old_string="original", new_string="renamed")])
        ],
    )
    errs = v.validate(plan)
    assert any(e.kind == "schema" and "context" in e.message for e in errs)


# ------------- Executor -------------

def test_executor_applies_modify(workspace: Path):
    exe = Executor(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "a.py", "original", "renamed",
            before="header\n", after="\nfooter",
        )],
    )
    result = exe.apply(plan)
    assert result.success, result
    assert (workspace / "a.py").read_text() == "header\nrenamed\nfooter\n"
    assert result.backup_dir is not None


def test_executor_rolls_back_on_failure(workspace: Path):
    exe = Executor(str(workspace))
    original_a = (workspace / "a.py").read_text()
    original_b = (workspace / "b.py").read_text()
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[
            _modify_patch(
                "a.py", "original", "renamed",
                before="header\n", after="\nfooter", pid="p1",
            ),
            _modify_patch(
                "b.py", "DOES_NOT_EXIST", "x",
                before="nothing\n", after="\nnothing", pid="p2",
            ),
        ],
    )
    result = exe.apply(plan)
    assert not result.success
    assert result.rolled_back
    assert (workspace / "a.py").read_text() == original_a
    assert (workspace / "b.py").read_text() == original_b


def test_executor_rollback_removes_created_file(workspace: Path):
    exe = Executor(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[
            Patch(id="p1", kind=PatchKind.CREATE, path="new.py",
                  rationale="r", content="hello\n"),
            _modify_patch(
                "a.py", "NO_SUCH", "x",
                before="nope\n", after="\nnope", pid="p2",
            ),
        ],
    )
    result = exe.apply(plan)
    assert not result.success
    assert not (workspace / "new.py").exists()


def test_executor_already_applied_on_rerun(workspace: Path):
    exe = Executor(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "a.py", "original", "renamed",
            before="header\n", after="\nfooter",
        )],
    )
    r1 = exe.apply(plan)
    assert r1.success
    r2 = exe.apply(plan)
    assert r2.success
    assert r2.results[0].status == PatchStatus.ALREADY_APPLIED


def test_executor_rollback_result_restores_previous(workspace: Path):
    exe = Executor(str(workspace))
    plan = PatchPlan(
        goal="g", strategy="s",
        patches=[_modify_patch(
            "a.py", "original", "renamed",
            before="header\n", after="\nfooter",
        )],
    )
    result = exe.apply(plan)
    assert result.success
    assert (workspace / "a.py").read_text() == "header\nrenamed\nfooter\n"
    exe.rollback_result(result)
    assert (workspace / "a.py").read_text() == "header\noriginal\nfooter\n"


# ------------- Pipeline (with fake provider) -------------

class FakeProvider:
    last_used_tools: tuple = ()

    def __init__(self, responses: list[str]):
        self._responses = list(responses)

    def generate(self, prompt: str, tools: tuple | None = None) -> str:
        self.last_used_tools = tuple(tools) if tools is not None else ()
        if not self._responses:
            raise RuntimeError("FakeProvider: no more responses")
        return self._responses.pop(0)


def _json_response(plan_dict: dict) -> str:
    return "```json\n" + json.dumps(plan_dict) + "\n```"


def test_pipeline_single_iteration_success(workspace: Path):
    from aegis.runtime import pipeline

    response = _json_response({
        "goal": "rename",
        "strategy": "single modify",
        "target_files": ["a.py"],
        "patches": [{
            "id": "p1", "kind": "modify", "path": "a.py", "rationale": "r",
            "edits": [{
                "old_string": "original", "new_string": "renamed",
                "context_before": "header\n", "context_after": "\nfooter",
            }],
        }],
        "done": True,
    })
    provider = FakeProvider([response])
    result = pipeline.run(
        task="rename", root=str(workspace), provider=provider,
        max_iters=2, include_file_snippets=False,
    )
    assert result.success, result
    assert (workspace / "a.py").read_text() == "header\nrenamed\nfooter\n"


def test_pipeline_stops_on_identical_plan(workspace: Path):
    from aegis.runtime import pipeline

    bad_plan = _json_response({
        "goal": "g", "strategy": "s", "target_files": ["a.py"],
        "patches": [{
            "id": "p1", "kind": "modify", "path": "a.py", "rationale": "r",
            "edits": [{
                "old_string": "NO_MATCH", "new_string": "x",
                "context_before": "nope\n", "context_after": "\nnope",
            }],
        }],
        "done": False,
    })
    provider = FakeProvider([bad_plan, bad_plan, bad_plan])
    result = pipeline.run(
        task="t", root=str(workspace), provider=provider,
        max_iters=5, include_file_snippets=False,
    )
    assert not result.success
    assert result.error is not None and "stalemate" in result.error


def test_pipeline_max_iters(workspace: Path):
    from aegis.runtime import pipeline

    responses = [
        _json_response({
            "goal": "g", "strategy": f"try {i}",
            "target_files": ["a.py"],
            "patches": [{
                "id": f"p{i}", "kind": "modify", "path": "a.py", "rationale": "r",
                "edits": [{
                    "old_string": f"MISS_{i}", "new_string": "x",
                    "context_before": f"ctx_{i}\n", "context_after": f"\nend_{i}",
                }],
            }],
            "done": False,
        })
        for i in range(3)
    ]
    provider = FakeProvider(responses)
    result = pipeline.run(
        task="t", root=str(workspace), provider=provider,
        max_iters=3, include_file_snippets=False,
    )
    assert not result.success
    assert result.iterations == 3
