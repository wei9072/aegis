"""
Example 03 — Layer C: defining your own task verifier.

The pipeline's `pipeline_success` only tells you "did the loop reach
plan.done?". It does NOT tell you whether the task itself was solved.
That's the role of the Layer C verifier — an external observer that
inspects the final workspace and reports SOLVED / INCOMPLETE /
ABANDONED.

Critical contract: the verifier ONLY writes to PipelineResult.task_verdict.
It NEVER feeds back into the planner prompt or the loop's decisions.
This keeps Aegis a decision-system rather than a goal-seeker. See
docs/v1_validation.md#framing for the full design rule.

Run from the repo root:
    python examples/03_custom_verifier.py
"""
import ast
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from aegis.agents.gemini import GeminiProvider  # noqa: E402
from aegis.runtime import pipeline  # noqa: E402
from aegis.runtime.task_verifier import VerifierResult  # noqa: E402


class FileParsesVerifier:
    """Trivial verifier: passes iff a target file is a valid Python module."""

    def __init__(self, target_relpath: str):
        self.target_relpath = target_relpath

    def verify(self, workspace: Path, trace) -> VerifierResult:
        target = workspace / self.target_relpath
        if not target.exists():
            return VerifierResult(passed=False, rationale=f"{self.target_relpath} missing")
        src = target.read_text(encoding="utf-8")
        try:
            ast.parse(src)
            return VerifierResult(
                passed=True,
                rationale=f"{self.target_relpath} parses cleanly",
                evidence={"bytes": len(src)},
            )
        except SyntaxError as e:
            return VerifierResult(
                passed=False,
                rationale=f"SyntaxError at line {e.lineno}: {e.msg}",
            )


def main() -> None:
    workspace = Path(__file__).parent / "_scratch_custom_verifier"
    workspace.mkdir(exist_ok=True)
    (workspace / "broken.py").write_text("def add(a, b)\n    return a + b\n", encoding="utf-8")

    result = pipeline.run(
        task="Fix the syntax error in broken.py minimally.",
        root=str(workspace),
        provider=GeminiProvider(model_name="gemma-4-31b-it"),
        verifier=FileParsesVerifier("broken.py"),
    )

    v = result.task_verdict
    print(f"task verdict: {v.pattern.value}")
    if v.verifier_result:
        print(f"  rationale: {v.verifier_result.rationale}")
    print(f"pipeline_success: {result.success} (this is a separate signal!)")


if __name__ == "__main__":
    main()
