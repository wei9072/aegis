"""
LLMPlanner: PlanContext -> PatchPlan.

Builds a structured prompt from context + signals + previous errors,
calls the LLM provider, extracts JSON, and parses into a PatchPlan.
Retries on parse failure (bounded).
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass, field

from aegis.agents.llm_adapter import LLMProvider
from aegis.core.bindings import Signal
from aegis.ir.patch import PatchPlan, plan_from_dict
from aegis.runtime.executor import ExecutionResult
from aegis.runtime.validator import ValidationError


@dataclass
class PlanContext:
    task: str
    root: str
    scope: list[str] | None = None
    py_files: list[str] = field(default_factory=list)
    signals: dict[str, list[Signal]] = field(default_factory=dict)
    graph_edges: list[tuple[str, str]] = field(default_factory=list)
    has_cycle: bool = False
    file_snippets: dict[str, str] = field(default_factory=dict)
    previous_plan: PatchPlan | None = None
    previous_errors: list[ValidationError] = field(default_factory=list)
    previous_result: ExecutionResult | None = None
    previous_regressed: bool = False


_PLAN_SCHEMA_HINT = """\
{
  "goal": "<your understanding of the user's task>",
  "strategy": "<one-paragraph approach>",
  "target_files": ["relative/path.py", "..."],
  "patches": [
    {
      "id": "p1",
      "kind": "modify",          // "create" | "modify" | "delete"
      "path": "relative/path.py",
      "rationale": "why this patch",
      "content": "<full file body, CREATE only>",
      "edits": [
        {
          "old_string": "<exact text to find, must be unique in the file>",
          "new_string": "<replacement>",
          "context_before": "<>=1 line of surrounding text above old_string>",
          "context_after":  "<>=1 line of surrounding text below old_string>"
        }
      ]
    }
  ],
  "done": false   // set true when you believe the task is complete
}
"""


class LLMPlanner:
    def __init__(self, provider: LLMProvider, max_retries: int = 2) -> None:
        self.provider = provider
        self.max_retries = max_retries

    def plan(self, ctx: PlanContext) -> PatchPlan:
        prompt = self._format_prompt(ctx)
        last_error: str | None = None
        for attempt in range(self.max_retries + 1):
            raw = self.provider.generate(
                prompt if attempt == 0 else self._format_parse_retry(prompt, last_error)
            )
            try:
                data = self._extract_json(raw)
                return plan_from_dict(data)
            except Exception as e:
                last_error = f"{type(e).__name__}: {e}"
        raise RuntimeError(
            f"Planner failed to produce valid PatchPlan JSON after "
            f"{self.max_retries + 1} attempts. Last error: {last_error}"
        )

    def _format_prompt(self, ctx: PlanContext) -> str:
        parts: list[str] = []
        parts.append("# Aegis Refactor Planner")
        parts.append(
            "You are an architecture-aware refactoring planner. Produce a "
            "structured PatchPlan (JSON) that makes incremental progress toward "
            "the user's task. Each MODIFY edit MUST include context_before / "
            "context_after surrounding the old_string so the change can be "
            "located unambiguously even if the code shifts."
        )
        parts.append("\n## Task")
        parts.append(ctx.task)

        if ctx.scope:
            parts.append("\n## Scope (patches MUST stay inside these paths)")
            for s in ctx.scope:
                parts.append(f"- {s}")

        parts.append("\n## Project files")
        for f in ctx.py_files[:200]:
            parts.append(f"- {f}")

        if ctx.has_cycle:
            parts.append("\n## Dependency cycle detected (Ring 0)")
            parts.append("The project currently has a circular import; "
                         "breaking it is high priority.")

        if ctx.signals:
            parts.append("\n## Structural signals (Ring 0.5)")
            for path, sigs in ctx.signals.items():
                if not sigs:
                    continue
                parts.append(f"\n### {path}")
                for s in sigs:
                    parts.append(f"- {s.name} = {s.value:.0f}  ({s.description})")

        if ctx.file_snippets:
            parts.append("\n## File contents")
            for path, body in ctx.file_snippets.items():
                parts.append(f"\n### {path}")
                parts.append("```python")
                parts.append(body)
                parts.append("```")

        if ctx.previous_plan is not None:
            parts.append("\n## Previous attempt")
            parts.append(f"Strategy: {ctx.previous_plan.strategy}")
            if ctx.previous_errors:
                parts.append("Validator errors to fix:")
                for err in ctx.previous_errors:
                    loc = f"patch={err.patch_id}"
                    if err.edit_index is not None:
                        loc += f", edit={err.edit_index}"
                    if err.matches:
                        loc += f", matches={err.matches}"
                    parts.append(f"- [{err.kind}] {loc}: {err.message}")
            if ctx.previous_regressed:
                parts.append(
                    "Previous plan APPLIED but was reverted because it increased "
                    "the total signal count (regression). Try a different approach "
                    "that does not add new structural issues."
                )
            elif ctx.previous_result is not None and not ctx.previous_result.success:
                parts.append("Execution failures:")
                for r in ctx.previous_result.results:
                    if r.error or r.status.value not in ("applied", "already_applied"):
                        parts.append(
                            f"- patch={r.patch_id} status={r.status.value} "
                            f"matches={r.matches} err={r.error or ''}"
                        )
            parts.append(
                "Produce a revised plan. If matches>1, expand context_before / "
                "context_after until the anchor is unique. If previous edits "
                "were correct, set done=true and return an empty patches list."
            )

        parts.append("\n## Output")
        parts.append("Return ONLY a fenced JSON block matching this schema:")
        parts.append("```json")
        parts.append(_PLAN_SCHEMA_HINT)
        parts.append("```")
        return "\n".join(parts)

    def _format_parse_retry(self, original: str, error: str | None) -> str:
        return (
            f"{original}\n\n"
            f"Previous response could not be parsed as a PatchPlan: {error}\n"
            "Return ONLY the JSON block. No prose, no explanation outside the block."
        )

    def _extract_json(self, text: str) -> dict:
        match = re.search(r"```json\s*(.*?)\s*```", text, re.DOTALL)
        if match:
            payload = match.group(1)
        else:
            start = text.find("{")
            end = text.rfind("}")
            if start == -1 or end == -1 or end < start:
                raise ValueError("no JSON object found in response")
            payload = text[start : end + 1]
        data = json.loads(payload)
        if not isinstance(data, dict):
            raise ValueError("top-level JSON is not an object")
        return data
