"""
PatchPlan: structured intent shared between Planner and Executor.

A PatchPlan is the contract: Planner produces it, Validator verifies it,
Executor applies it. Pure data — no I/O, no logic.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class PatchKind(str, Enum):
    CREATE = "create"
    MODIFY = "modify"
    DELETE = "delete"


class PatchStatus(str, Enum):
    APPLIED = "applied"
    ALREADY_APPLIED = "already_applied"
    NOT_FOUND = "not_found"
    AMBIGUOUS = "ambiguous"


@dataclass
class Edit:
    old_string: str
    new_string: str
    context_before: str = ""
    context_after: str = ""


@dataclass
class Patch:
    id: str
    kind: PatchKind
    path: str
    rationale: str = ""
    content: str | None = None
    edits: list[Edit] = field(default_factory=list)


@dataclass
class PatchPlan:
    goal: str
    strategy: str
    patches: list[Patch] = field(default_factory=list)
    target_files: list[str] = field(default_factory=list)
    done: bool = False
    iteration: int = 0
    parent_id: str | None = None


def patch_to_dict(patch: Patch) -> dict:
    return {
        "id": patch.id,
        "kind": patch.kind.value,
        "path": patch.path,
        "rationale": patch.rationale,
        "content": patch.content,
        "edits": [
            {
                "old_string": e.old_string,
                "new_string": e.new_string,
                "context_before": e.context_before,
                "context_after": e.context_after,
            }
            for e in patch.edits
        ],
    }


def plan_to_dict(plan: PatchPlan) -> dict:
    return {
        "goal": plan.goal,
        "strategy": plan.strategy,
        "patches": [patch_to_dict(p) for p in plan.patches],
        "target_files": plan.target_files,
        "done": plan.done,
        "iteration": plan.iteration,
        "parent_id": plan.parent_id,
    }


def patch_from_dict(data: dict) -> Patch:
    return Patch(
        id=data["id"],
        kind=PatchKind(data["kind"]),
        path=data["path"],
        rationale=data.get("rationale", ""),
        content=data.get("content"),
        edits=[
            Edit(
                old_string=e["old_string"],
                new_string=e["new_string"],
                context_before=e.get("context_before", ""),
                context_after=e.get("context_after", ""),
            )
            for e in data.get("edits", [])
        ],
    )


def plan_from_dict(data: dict) -> PatchPlan:
    return PatchPlan(
        goal=data["goal"],
        strategy=data["strategy"],
        patches=[patch_from_dict(p) for p in data.get("patches", [])],
        target_files=data.get("target_files", []),
        done=data.get("done", False),
        iteration=data.get("iteration", 0),
        parent_id=data.get("parent_id"),
    )
